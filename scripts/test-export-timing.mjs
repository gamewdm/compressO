import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import ts from 'typescript'

function load(file, dependencies) {
  const { outputText } = ts.transpileModule(
    readFileSync(new URL(`../src/${file}`, import.meta.url), 'utf8'),
    {
      compilerOptions: {
        module: ts.ModuleKind.CommonJS,
        jsx: ts.JsxEmit.ReactJSX,
      },
    },
  )
  const exports = {}
  new Function('require', 'exports', outputText)((name) => {
    assert.ok(name in dependencies, `Unexpected dependency: ${name}`)
    return dependencies[name]
  }, exports)
  return exports
}

async function setup({ immediate = false, ask = false, fail = false } = {}) {
  const moves = []
  const folders = []
  const errors = []
  const listeners = new Map()
  const settings = {
    exportEachWhenFinished: immediate,
    saveLargerOutputs: true,
    preserveFolderStructure: true,
  }
  const app = {
    state: { batchId: 'batch', isCompressing: true, media: [] },
    takeSnapshot() {
      this.savedSnapshot = structuredClone(this.state)
    },
  }
  const snapshot = (value) =>
    value === app
      ? { state: structuredClone(app.state) }
      : structuredClone(value)
  const core = { convertFileSrc: (path) => path }
  const output = {
    saveLocation: ask ? 'ask' : 'fixedFolder',
    saveFolder: 'D:/exports',
    filePrefix: 'compressed-',
  }
  let activeMoves = 0
  let maxActiveMoves = 0
  const saving = load('routes/(root)/-save.ts', {
    '@tauri-apps/api': { core },
    '@tauri-apps/api/path': {
      appDataDir: async () => 'C:/profile',
      join: async (...parts) => parts.join('/'),
    },
    '@tauri-apps/plugin-dialog': {},
    valtio: { snapshot },
    '@/components/Toast': {
      toast: { error: (message) => errors.push(message), info() {} },
    },
    '@/i18n': { t: (key) => key },
    '@/stores/settingsStore': { settingsStore: settings },
    '@/tauri/commands/fs': {
      getAvailablePath: async (path) => {
        folders.push(path)
        return `${path} (1)`
      },
      moveFile: async (source, destination) => {
        activeMoves++
        maxActiveMoves = Math.max(maxActiveMoves, activeMoves)
        await new Promise((resolve) => setImmediate(resolve))
        activeMoves--
        if (fail) throw Error('drive unavailable')
        moves.push({ source, destination })
        return destination
      },
    },
    '@/types/app': {},
    '@/utils/fs': {
      formatBytes: String,
      getParentDirectory: (path) => path.slice(0, path.lastIndexOf('/')),
    },
    '@/utils/presetOutput': {
      getPresetOutput: () => output,
      savesAutomatically: (value) => value.saveLocation !== 'ask',
    },
    './-history': { recordSavedOutput() {} },
    './-state': { appProxy: app },
  })
  const progress = load('routes/(root)/ui/CompressionProgress.tsx', {
    '@tauri-apps/api': {
      core,
      event: {
        listen: async (name, fn) => {
          listeners.set(name, fn)
          return () => {}
        },
      },
    },
    '@tauri-apps/api/core': { invoke: async () => {} },
    'lodash/clamp': { default: (n) => n },
    react: { memo: (fn) => fn, useEffect: (fn) => fn(), useRef: () => ({}) },
    valtio: { snapshot, useSnapshot: (value) => value },
    '@/stores/settingsStore': { settingsStore: settings },
    '@/types/compression': {
      CustomEvents: {
        BatchMediaCompressionProgress: 'progress',
        BatchMediaIndividualCompressionCompletion: 'completed',
      },
    },
    '@/utils/fs': { formatBytes: String, getOutputFileName: (name) => name },
    '@/utils/string': {},
    '../-compression': { getOutputOptions: () => ({}) },
    '../-save': saving,
    '../-state': { appProxy: app },
  })
  progress.default()
  await Promise.resolve()
  function add(id) {
    app.state.media.push({
      id,
      type: 'video',
      fileName: `${id}.mp4`,
      pathRaw: `D:/input/${id}.mp4`,
      sizeInBytes: 100,
      relativeDir: 'Vacation/Day1',
    })
  }
  function complete(id) {
    listeners.get('completed')({
      payload: {
        batchId: 'batch',
        result: {
          mediaType: 'video',
          videoId: id,
          fileMetadata: {
            path: `D:/work/${id}.mp4`,
            fileName: `${id}.mp4`,
            extension: 'mp4',
            size: 50,
          },
        },
      },
    })
  }
  add('a')
  add('b')
  return {
    app,
    add,
    complete,
    moves,
    folders,
    errors,
    saving,
    maxActiveMoves: () => maxActiveMoves,
    retry: () => {
      fail = false
    },
  }
}

test('default keeps outputs cached until the whole queue is saved', async () => {
  const h = await setup()
  h.complete('a')
  await h.saving.waitForAutomaticSaves()
  assert.equal(h.moves.length, 0)
  h.complete('b')
  await h.saving.autoSaveCompressedMedia()
  assert.equal(h.moves.length, 2)
})

test('immediate exports only the finished file while the next video is pending', async () => {
  const h = await setup({ immediate: true })
  h.complete('a')
  await h.saving.waitForAutomaticSaves()
  assert.deepEqual(
    h.moves.map((move) => move.source),
    ['D:/work/a.mp4'],
  )
  assert.equal(h.app.state.media[1].compressedFile, undefined)
  assert.equal(h.app.state.isCompressing, true)
  assert.equal(h.app.savedSnapshot.media[0].compressedFile.isSaved, true)
  assert.equal(
    h.app.savedSnapshot.media[0].compressedFile.savedPath,
    h.moves[0].destination,
  )
})

test('concurrent completions and newly added files share a folder without duplicate exports', async () => {
  const h = await setup({ immediate: true })
  h.complete('a')
  h.complete('b')
  await h.saving.waitForAutomaticSaves()
  h.add('c')
  h.complete('c')
  await h.saving.autoSaveCompressedMedia()
  assert.equal(h.moves.length, 3)
  assert.equal(h.maxActiveMoves(), 1)
  assert.deepEqual(h.folders, ['D:/exports/Vacation'])
  assert.ok(
    h.moves.every((move) =>
      move.destination.startsWith('D:/exports/Vacation (1)/Day1/'),
    ),
  )
  h.complete('a') // A late duplicate completion must retain the saved flags.
  await h.saving.autoSaveCompressedMedia()
  assert.equal(h.moves.length, 3)
})

test('ask every time stays manual in immediate mode', async () => {
  const h = await setup({ immediate: true, ask: true })
  h.complete('a')
  h.complete('b')
  await h.saving.autoSaveCompressedMedia()
  assert.equal(h.moves.length, 0)
  assert.equal(h.app.state.media[0].compressedFile.isSaved, undefined)
})

test('failed exports remain unsaved and can be retried at queue end', async () => {
  const h = await setup({ immediate: true, fail: true })
  h.complete('a')
  await h.saving.waitForAutomaticSaves()
  assert.equal(h.app.state.media[0].compressedFile.isSaved, undefined)
  assert.equal(h.app.state.media[0].compressedFile.isSaving, false)
  assert.equal(h.app.state.isSaving, false)
  assert.equal(h.errors.length, 1)
  h.retry()
  await h.saving.autoSaveCompressedMedia()
  assert.equal(h.app.state.media[0].compressedFile.isSaved, true)
})

test('cancellation waits for pending exports and retains saved paths in its restore snapshot', async () => {
  const h = await setup({ immediate: true })
  h.complete('a')
  await h.saving.waitForAutomaticSaves()
  h.app.state = structuredClone(h.app.savedSnapshot)
  h.app.state.isCompressing = false
  await h.saving.autoSaveCompressedMedia()
  assert.equal(h.moves.length, 1)
  assert.equal(
    h.app.state.media[0].compressedFile.savedPath,
    h.moves[0].destination,
  )
  assert.equal(h.app.state.media[1].compressedFile, undefined)
})

test('waiting for saves includes completions arriving during an export', async () => {
  const h = await setup({ immediate: true })
  h.complete('a')
  const pending = h.saving.waitForAutomaticSaves()
  await Promise.resolve()
  h.complete('b')
  await pending
  assert.equal(h.moves.length, 2)
  assert.equal(h.app.savedSnapshot.media[1].compressedFile.isSaved, true)
})

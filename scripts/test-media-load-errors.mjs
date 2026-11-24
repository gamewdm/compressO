import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import ts from 'typescript'

function load(file, dependencies = {}) {
  const { outputText } = ts.transpileModule(
    readFileSync(new URL(`../src/${file}`, import.meta.url), 'utf8'),
    { compilerOptions: { module: ts.ModuleKind.CommonJS } },
  )
  const exports = {}
  new Function('require', 'exports', outputText)(
    (name) => dependencies[name] ?? {}, exports,
  )
  return exports
}

const english = JSON.parse(readFileSync(new URL('../src/i18n/locales/en.json', import.meta.url)))
const promise = load('utils/promise.ts')

for (const failure of ['metadata', 'thumbnail', 'settings', 'timeout', 'small', 'permission', null]) {
  test(`import reports ${failure ?? 'success'} and keeps other queue items`, async () => {
    const messages = []
    const logs = []
    const denied = []
    const state = { media: [{ pathRaw: 'existing.mp4' }], isCompressing: false }
    const path = 'E:\\downloads\\test.mp4'
    const fail = () => Promise.reject('decoder not available')
    const { addMedia } = load('routes/(root)/-media.ts', {
      '@tauri-apps/api': { core: { convertFileSrc: (path) => path } },
      'lodash/cloneDeep': { default: (value) => structuredClone(value) },
      '@/components/Toast': { toast: { error: (...args) => messages.push(args) } },
      '@/i18n': { t: (key) => key.split('.').reduce((value, key) => value[key], english) },
      '@/tauri/commands/app': { logError: (...args) => logs.push(args) },
      '@/tauri/commands/fs': {
        getFileMetadata: async () => {
          if (failure === 'permission') throw 'PERMISSION_DENIED: blocked'
          return { mimeType: 'video/mp4', size: failure === 'small' ? 10 : 2000, fileName: 'test.mp4' }
        },
        isPermissionDeniedError: (error) => String(error).startsWith('PERMISSION_DENIED:'),
      },
      '@/tauri/commands/ffprobe': {
        getVideoBasicInfo: failure === 'metadata' ? fail : async () => ({ dimensions: [1920, 1080], duration: 10 }),
        getVideoDynamicRange: async () => undefined,
      },
      '@/tauri/commands/ffmpeg': {
        generateVideoThumbnail: failure === 'thumbnail' ? fail : async () => ({ id: 'new', filePath: 'thumb.jpg' }),
      },
      '@/types/compression': {
        inputExtensions: { video: ['mp4'], image: [], audio: [] },
        extensions: { video: {} }, isAudioFileExtension: () => false,
      },
      '@/utils/fs': { formatBytes: String },
      '@/utils/promise': {
        ...promise,
        withTimeout: failure === 'timeout'
          ? (operation) => { operation.catch(() => {}); return Promise.reject(new promise.TimeoutError()) }
          : (operation) => operation,
      },
      './-presets': { getInitialVideoConfig: failure === 'settings' ? fail : async () => ({ config: {} }) },
      './-state': { appProxy: { state }, videoConfigInitialState: {}, normalizeBatchMediaConfig() {} },
      './-toasts': { showAccessDeniedToast: (count) => denied.push(count) },
    })
    await addMedia(path)
    assert.equal(state.isLoadingMediaFiles, false)
    assert.equal(state.media[0].pathRaw, 'existing.mp4')
    if (!failure) {
      assert.equal(state.media.length, 2)
      assert.deepEqual(logs, [])
      assert.deepEqual(messages, [])
      return
    }
    assert.equal(state.media.length, 1)
    assert.equal(logs.length, 1)
    assert.ok(logs[0][0].includes(JSON.stringify(path)))
    if (failure === 'permission') {
      assert.deepEqual(denied, [1])
      assert.deepEqual(messages, [])
      return
    }
    assert.equal(messages.length, 1)
    const [title, options] = messages[0]
    assert.equal(title, english.media.corrupted)
    assert.ok(options.description.startsWith('test.mp4: '))
    const expected = {
      metadata: `${english.media.importMetadata}: decoder not available`,
      thumbnail: `${english.media.importThumbnail}: decoder not available`,
      settings: `${english.media.importSettings}: decoder not available`,
      timeout: english.media.importTimeout,
      small: english.media.importInvalid,
    }[failure]
    assert.ok(options.description.includes(expected))
    assert.equal(logs[0][1], expected)
    assert.equal(options.duration, Infinity)
    assert.equal(options.closeButton, true)
  })
}

import { SelectItem } from '@heroui/react'
import React from 'react'
import { Trans, useTranslation } from 'react-i18next'
import { useSnapshot } from 'valtio'

import Button from '@/components/Button'
import ColorPicker from '@/components/ColorPicker'
import InfoTooltip from '@/components/InfoTooltip'
import Select from '@/components/Select'
import SettingLink from '@/components/SettingLink'
import Slider from '@/components/Slider'
import Switch from '@/components/Switch'
import TextInput from '@/components/TextInput'
import { ThemeSwitcher } from '@/components/ThemeSwitcher'
import { toast } from '@/components/Toast'
import { useDropQueue } from '@/hooks/useDropQueue'
import { usePrimaryColor } from '@/hooks/usePrimaryColor'
import { isWindowTintSupported } from '@/hooks/useWindowTint'
import { LANGUAGES, type LanguageCode } from '@/i18n/languages'
import { settingsStore, TINT_OPACITY_RANGE } from '@/stores/settingsStore'
import {
  getJobLogSettings,
  getLogFilePath,
  getOpenAtLogin,
  type JobLogSettings,
  type OpenAtLoginState,
  openLoginItemsSettings,
  restartApp,
  setJobLogSettings,
  setOpenAtLogin,
} from '@/tauri/commands/app'
import { showItemInFileManager } from '@/tauri/commands/fs'
import { playCompletionSound as playSound } from '@/tauri/commands/sound'
import { getPlatform } from '@/utils/fs'
import { cn } from '@/utils/tailwind'
import { hasTray, TRAY_CONTEXT } from '@/utils/tray'
import { isVideoPreviewAvailable } from '@/utils/video'
import { SettingsRow, SettingsSection } from './SettingsLayout'

const { isLinux } = getPlatform()

function JobLogSettingsRows() {
  const { t } = useTranslation()
  const [settings, setSettings] = React.useState<JobLogSettings | null>(null)
  const [keptJobs, setKeptJobs] = React.useState('')
  const [isSaving, setIsSaving] = React.useState(false)
  const key = 'settings.general.troubleshooting.jobLogs' as const

  React.useEffect(() => {
    const read = () => {
      getJobLogSettings()
        .then((value) => {
          setSettings(value)
          setKeptJobs(String(value.keptJobs))
        })
        .catch(() => toast.error(t(`${key}.failed`)))
    }
    read()
    window.addEventListener('focus', read)
    return () => window.removeEventListener('focus', read)
  }, [t, key])

  const save = async (next: JobLogSettings) => {
    setIsSaving(true)
    try {
      await setJobLogSettings(next)
      setSettings(next)
      setKeptJobs(String(next.keptJobs))
    } catch {
      setKeptJobs(String(settings?.keptJobs ?? ''))
      toast.error(t(`${key}.failed`))
    } finally {
      setIsSaving(false)
    }
  }

  const saveCount = () => {
    if (!settings || isSaving) return
    const count = Number(keptJobs)
    if (!Number.isInteger(count) || count < 1 || count > 1000) {
      setKeptJobs(String(settings.keptJobs))
      toast.error(t(`${key}.invalidCount`))
    } else if (count !== settings.keptJobs) {
      void save({ ...settings, keptJobs: count })
    }
  }

  return (
    <>
      <SettingsRow
        settingId="jobLogInterval"
        title={t(`${key}.intervalTitle`)}
        description={t(`${key}.intervalDescription`)}
      >
        <Select
          aria-label={t(`${key}.intervalTitle`)}
          selectedKeys={settings ? [String(settings.intervalSeconds)] : []}
          disallowEmptySelection
          isDisabled={!settings || isSaving}
          onSelectionChange={(keys) => {
            const [value] = Array.from(keys)
            if (settings && value) {
              void save({ ...settings, intervalSeconds: Number(value) })
            }
          }}
          size="sm"
          className="w-[200px]"
        >
          {([30, 60, 300, 600] as const).map((seconds) => (
            <SelectItem key={seconds}>
              {t(`${key}.interval${seconds}`)}
            </SelectItem>
          ))}
        </Select>
      </SettingsRow>
      <SettingsRow
        settingId="jobLogRetention"
        title={t(`${key}.retentionTitle`)}
        description={t(`${key}.retentionDescription`)}
      >
        <TextInput
          aria-label={t(`${key}.retentionTitle`)}
          type="number"
          min={1}
          max={1000}
          step={1}
          value={keptJobs}
          isDisabled={!settings || isSaving}
          onValueChange={setKeptJobs}
          onBlur={saveCount}
          onKeyDown={(event) => {
            if (event.key === 'Enter') event.currentTarget.blur()
          }}
          className="w-[100px]"
        />
      </SettingsRow>
    </>
  )
}

/** Opening the app at login, which the system keeps (also changed in its login items) */
function StartupSection() {
  const { t } = useTranslation()
  const { openInMenuBarOnly, showMenuBarIcon } = useSnapshot(settingsStore)
  // `null` until read from the system
  const [state, setState] = React.useState<OpenAtLoginState | null>(null)
  const isOpenAtLogin = !!state?.isEnabled

  const readState = React.useCallback(() => {
    getOpenAtLogin()
      .then(setState)
      .catch(() => setState({ isEnabled: false, needsApproval: false }))
  }, [])

  // Read again when coming back to the app, e.g. after changing it in the system's settings
  React.useEffect(() => {
    readState()
    window.addEventListener('focus', readState)
    return () => window.removeEventListener('focus', readState)
  }, [readState])

  const handleOpenAtLoginChange = async (isSelected: boolean) => {
    setState((current) => ({
      isEnabled: isSelected,
      needsApproval: current?.needsApproval ?? false,
    }))
    try {
      await setOpenAtLogin(isSelected)
    } catch {
      toast.error(t('settings.general.startup.changeFailed'))
    }
    // As the system has it (e.g. still switched off in its settings)
    readState()
  }

  return (
    <SettingsSection title={t('settings.general.startup.title')}>
      <SettingsRow
        settingId="openAtLogin"
        title={t('settings.general.startup.openAtLogin.title')}
        description={t('settings.general.startup.openAtLogin.description')}
      >
        <Switch
          aria-label={t('settings.general.startup.openAtLogin.title')}
          isSelected={isOpenAtLogin}
          isDisabled={state == null}
          onValueChange={handleOpenAtLoginChange}
        />
      </SettingsRow>
      <SettingsRow
        isVisible={state?.needsApproval}
        isNested
        title={t('settings.general.startup.needsApproval.title')}
        description={t('settings.general.startup.needsApproval.description')}
      >
        <Button
          size="sm"
          onPress={() => {
            openLoginItemsSettings().catch(() => {})
          }}
        >
          {t('settings.general.startup.needsApproval.button')}
        </Button>
      </SettingsRow>
      <SettingsRow
        settingId="openInTrayOnly"
        isVisible={hasTray && isOpenAtLogin}
        isNested
        title={t('settings.general.startup.openInTrayOnly.title', {
          context: TRAY_CONTEXT,
        })}
        description={t('settings.general.startup.openInTrayOnly.description', {
          context: TRAY_CONTEXT,
        })}
      >
        <div className="flex items-center gap-2">
          <InfoTooltip
            content={
              !showMenuBarIcon && (
                <Trans
                  i18nKey="settings.general.startup.openInTrayOnly.needsTrayIcon"
                  context={TRAY_CONTEXT}
                  components={{
                    setting: (
                      <SettingLink target={{ settingId: 'showTrayIcon' }} />
                    ),
                  }}
                />
              )
            }
          />
          <Switch
            aria-label={t('settings.general.startup.openInTrayOnly.title', {
              context: TRAY_CONTEXT,
            })}
            isSelected={openInMenuBarOnly && showMenuBarIcon}
            isDisabled={!showMenuBarIcon}
            onValueChange={(isSelected) => {
              settingsStore.openInMenuBarOnly = isSelected
            }}
          />
        </div>
      </SettingsRow>
    </SettingsSection>
  )
}

/** The language of the interface (English by default) */
function LanguageSelect() {
  const { t } = useTranslation()
  const { language } = useSnapshot(settingsStore)

  return (
    <Select
      aria-label={t('settings.general.appearance.language.title')}
      selectedKeys={[language]}
      disallowEmptySelection
      onSelectionChange={(keys) => {
        const [key] = Array.from(keys)
        if (key) settingsStore.language = key as LanguageCode
      }}
      size="sm"
      className="w-[200px]"
    >
      {LANGUAGES.map((item) => (
        <SelectItem key={item.code}>{item.name}</SelectItem>
      ))}
    </Select>
  )
}

function General() {
  const { t } = useTranslation()
  const { color, setColor } = usePrimaryColor()
  const {
    tintedWindow,
    tintOpacity,
    showMenuBarIcon,
    playCompletionSound,
    enableVideoPreviews,
    enableAudioWaveforms,
    preventSleepWhileCompressing,
    keepScreenOnWhileCompressing,
  } = useSnapshot(settingsStore)
  // Read when the app starts, so a change applies after a restart
  const needsRestart =
    isLinux && enableVideoPreviews !== isVideoPreviewAvailable
  // The drop zone's queue is compressed by the menu bar, so its icon stays until the queue is done
  // (or stopped)
  const isDropQueueActive = useDropQueue().some(
    (item) => item.status === 'waiting' || item.status === 'compressing',
  )
  const isMenuBarIconLocked = showMenuBarIcon && isDropQueueActive

  return (
    <>
      <SettingsSection title={t('settings.general.appearance.title')}>
        <SettingsRow
          settingId="language"
          title={t('settings.general.appearance.language.title')}
          description={t('settings.general.appearance.language.description')}
        >
          <LanguageSelect />
        </SettingsRow>
        <SettingsRow
          settingId="theme"
          title={t('settings.general.appearance.theme.title')}
          description={t('settings.general.appearance.theme.description')}
        >
          <ThemeSwitcher />
        </SettingsRow>
        <SettingsRow
          settingId="accentColor"
          title={t('settings.general.appearance.accentColor.title')}
          description={t('settings.general.appearance.accentColor.description')}
        >
          <ColorPicker color={color} onChange={setColor} />
        </SettingsRow>
        {isWindowTintSupported ? (
          <SettingsRow
            settingId="tintedWindow"
            title={t('settings.general.appearance.tintedWindow.title')}
            description={t(
              'settings.general.appearance.tintedWindow.description',
            )}
          >
            <Switch
              aria-label={t('settings.general.appearance.tintedWindow.title')}
              isSelected={tintedWindow}
              onValueChange={(isSelected) => {
                settingsStore.tintedWindow = isSelected
              }}
            />
          </SettingsRow>
        ) : null}
        <SettingsRow
          settingId="tintOpacity"
          isVisible={isWindowTintSupported && tintedWindow}
          isNested
          title={t('settings.general.appearance.tintOpacity.title')}
          description={t('settings.general.appearance.tintOpacity.description')}
        >
          <div className="flex items-center gap-3 w-[220px]">
            <Slider
              aria-label={t('settings.general.appearance.tintOpacity.title')}
              size="sm"
              minValue={TINT_OPACITY_RANGE.min}
              maxValue={TINT_OPACITY_RANGE.max}
              step={5}
              value={tintOpacity}
              onChange={(value) => {
                settingsStore.tintOpacity = Array.isArray(value)
                  ? value[0]
                  : value
              }}
            />
          </div>
        </SettingsRow>
      </SettingsSection>
      {hasTray ? (
        <SettingsSection
          title={t('settings.general.tray.title', { context: TRAY_CONTEXT })}
        >
          <SettingsRow
            settingId="showTrayIcon"
            title={t('settings.general.tray.showIcon.title', {
              context: TRAY_CONTEXT,
            })}
            description={t('settings.general.tray.showIcon.description', {
              context: TRAY_CONTEXT,
            })}
          >
            <div className="flex items-center gap-2">
              <InfoTooltip
                content={
                  isMenuBarIconLocked &&
                  t('settings.general.tray.showIcon.locked')
                }
              />
              <Switch
                aria-label={t('settings.general.tray.showIcon.title', {
                  context: TRAY_CONTEXT,
                })}
                isSelected={showMenuBarIcon}
                isDisabled={isMenuBarIconLocked}
                onValueChange={(isSelected) => {
                  settingsStore.showMenuBarIcon = isSelected
                }}
              />
            </div>
          </SettingsRow>
        </SettingsSection>
      ) : null}
      {isLinux ? (
        <SettingsSection title={t('settings.general.preview.title')}>
          <SettingsRow
            settingId="videoPreviews"
            isVisible={isLinux}
            title={t('settings.general.preview.videoPreviews.title')}
            description={t(
              'settings.general.preview.videoPreviews.description',
            )}
          >
            <div className="flex items-center gap-2">
              {/* Its space is always kept & it fades in, so the switch does not move */}
              <Button
                size="sm"
                onPress={() => {
                  restartApp().catch(() => {})
                }}
                isDisabled={!needsRestart}
                aria-hidden={!needsRestart}
                tabIndex={needsRestart ? undefined : -1}
                className={cn(
                  'transition-opacity duration-200 data-[disabled=true]:opacity-0',
                  !needsRestart && 'pointer-events-none',
                )}
              >
                {t('settings.general.preview.restart')}
              </Button>
              <Switch
                aria-label={t('settings.general.preview.videoPreviews.title')}
                isSelected={enableVideoPreviews}
                onValueChange={(isSelected) => {
                  settingsStore.enableVideoPreviews = isSelected
                }}
              />
            </div>
          </SettingsRow>
        </SettingsSection>
      ) : null}

      <StartupSection />
      <SettingsSection title={t('settings.general.sleep.title')}>
        <SettingsRow
          settingId="preventSleep"
          title={t('settings.general.sleep.preventSleep.title')}
          description={t('settings.general.sleep.preventSleep.description')}
        >
          <Switch
            aria-label={t('settings.general.sleep.preventSleep.title')}
            isSelected={preventSleepWhileCompressing}
            onValueChange={(isSelected) => {
              settingsStore.preventSleepWhileCompressing = isSelected
            }}
          />
        </SettingsRow>
        <SettingsRow
          settingId="keepScreenOn"
          isVisible={preventSleepWhileCompressing}
          isNested
          title={t('settings.general.sleep.keepScreenOn.title')}
          description={t('settings.general.sleep.keepScreenOn.description')}
        >
          <Switch
            aria-label={t('settings.general.sleep.keepScreenOn.title')}
            isSelected={keepScreenOnWhileCompressing}
            onValueChange={(isSelected) => {
              settingsStore.keepScreenOnWhileCompressing = isSelected
            }}
          />
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title={t('settings.general.sound.title')}>
        <SettingsRow
          settingId="audioWaveforms"
          // Drawn over the player, which Linux only has with video previews on
          isVisible={!isLinux || enableVideoPreviews}
          title={t('settings.general.sound.audioWaveforms.title')}
          description={t('settings.general.sound.audioWaveforms.description')}
        >
          <Switch
            aria-label={t('settings.general.sound.audioWaveforms.title')}
            isSelected={enableAudioWaveforms}
            onValueChange={(value) => {
              settingsStore.enableAudioWaveforms = value
            }}
          />
        </SettingsRow>
        <SettingsRow
          settingId="completionSound"
          title={t('settings.general.sound.completionSound.title')}
          description={t('settings.general.sound.completionSound.description')}
        >
          <Switch
            aria-label={t('settings.general.sound.completionSound.title')}
            isSelected={playCompletionSound}
            onValueChange={(isSelected) => {
              settingsStore.playCompletionSound = isSelected
              // A preview of the sound that will play
              if (isSelected) playSound().catch(() => {})
            }}
          />
        </SettingsRow>
      </SettingsSection>
      <SettingsSection title={t('settings.general.troubleshooting.title')}>
        <JobLogSettingsRows />
        <SettingsRow
          settingId="logFiles"
          title={t('settings.general.troubleshooting.logFiles.title')}
          description={t(
            'settings.general.troubleshooting.logFiles.description',
          )}
        >
          <Button
            size="sm"
            onPress={async () => {
              try {
                await showItemInFileManager(await getLogFilePath())
              } catch {
                toast.error(
                  t('settings.general.troubleshooting.logFiles.failed'),
                )
              }
            }}
          >
            {t('settings.general.troubleshooting.logFiles.button')}
          </Button>
        </SettingsRow>
      </SettingsSection>
    </>
  )
}

export default General

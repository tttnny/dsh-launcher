<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { LauncherUpdateInfo, LogLevel, ThemeMode } from '@/api/types'
import { useAction } from '@/composables/useAction'
import { SUPPORTED_LOCALES } from '@/i18n'
import { useLauncherStore } from '@/stores/launcher'
import { SHORTCUT_DOCS } from '@/shortcuts'

const router = useRouter()

/** The setup page doubles as the toolchain detail view; Settings is its
 *  permanent entry point (the page is otherwise only reached by the
 *  missing-Node redirect). */
function openEnvironmentCheck() {
  void router.push({ name: 'setup' })
}

const { t } = useI18n()
const store = useLauncherStore()

const THEME_OPTIONS = computed<{ value: ThemeMode; label: string }[]>(() => [
  { value: 'light', label: t('settings.theme.light') },
  { value: 'dark', label: t('settings.theme.dark') },
  { value: 'system', label: t('settings.theme.system') },
])

const LOG_LEVEL_OPTIONS = computed<{ value: LogLevel; label: string }[]>(() => [
  { value: 'debug', label: t('settings.logLevel.debug') },
  { value: 'info', label: t('settings.logLevel.info') },
  { value: 'warn', label: t('settings.logLevel.warn') },
  { value: 'error', label: t('settings.logLevel.error') },
])

const shortcutRows = computed<{ label: string; keys: string[]; native: boolean }[]>(() =>
  SHORTCUT_DOCS.map((d) => ({ label: t(d.labelKey), keys: d.keys, native: d.native ?? false })),
)

const saveAction = useAction(
  (patch: Parameters<typeof api.updateSettings>[0]) => api.updateSettings(patch),
  { success: () => t('settings.saved') },
)

async function patchSettings(patch: Parameters<typeof api.updateSettings>[0]) {
  const res = await saveAction.run(patch)
  if (res === undefined) return
  store.settings = res
}

async function onThemeChange(value: unknown) {
  await patchSettings({ theme: String(value) as ThemeMode })
}

async function onLogLevelChange(value: unknown) {
  await patchSettings({ log_level: String(value) as LogLevel })
}

const launcherVersion = ref('')
const updateInfo = ref<LauncherUpdateInfo | null>(null)
const updateChannel = ref<'dev' | 'release'>('dev')

const UPDATE_CHANNEL_OPTIONS = computed<{ value: 'dev' | 'release'; label: string }[]>(() => [
  { value: 'dev', label: t('settings.update.channel.dev') },
  { value: 'release', label: t('settings.update.channel.release') },
])

const dataDir = ref('')

onMounted(async () => {
  try {
    launcherVersion.value = await api.getLauncherVersion()
  } catch {
    launcherVersion.value = '?'
  }
  try {
    dataDir.value = await api.getLauncherDirectory()
  } catch {
    dataDir.value = ''
  }
})

const checkUpdateAction = useAction(() => api.checkLauncherUpdate(updateChannel.value))
const checkingUpdate = computed(() => checkUpdateAction.busy['*'])

async function onCheckUpdate() {
  const res = await checkUpdateAction.run()
  if (res === undefined) return
  updateInfo.value = res
  if (res.up_to_date) Message.success(t('settings.update.upToDate'))
}

async function onUpdateChannelChange(value: unknown) {
  const channel = String(value) === 'release' ? 'release' : 'dev'
  updateChannel.value = channel
  updateInfo.value = null
}

const openDataDirAction = useAction(() => api.openLauncherDirectory())

async function onOpenDataDir() {
  const dir = await openDataDirAction.run()
  if (dir === undefined) return
  dataDir.value = dir
}

const openLogAction = useAction(() => api.openLauncherLog(), {
  success: (path) => t('settings.logOpened', { path }),
})
const onOpenLauncherLog = openLogAction.run

async function onLocaleChange(value: unknown) {
  await patchSettings({ locale: String(value) })
}

async function onTrayChange(value: unknown) {
  await patchSettings({ minimize_to_tray: Boolean(value) })
}

async function onAutostartChange(value: unknown) {
  await patchSettings({ autostart: Boolean(value) })
}

const TERMINAL_OPTIONS = computed<{ value: string; label: string }[]>(() => [
  { value: 'system', label: t('settings.terminal.system') },
  { value: 'ghostty', label: t('settings.terminal.ghostty') },
])

async function onTerminalChange(value: unknown) {
  await patchSettings({ terminal: String(value) })
}

async function onProxyEnabledChange(value: unknown) {
  await patchSettings({ proxy_enabled: Boolean(value) })
}

async function onProxyApplyDshChange(value: unknown) {
  await patchSettings({ proxy_apply_dsh: Boolean(value) })
}

const proxyUrl = ref(store.settings.proxy_url ?? '')
const proxyPort = ref(store.settings.proxy_port ?? 7890)
const noProxy = ref(store.settings.no_proxy ?? '')

watch(
  () => [store.settings.proxy_url, store.settings.proxy_port, store.settings.no_proxy] as const,
  ([url, port, np]) => {
    const u = String(url ?? '')
    if (u !== proxyUrl.value) proxyUrl.value = u
    const p = Number(port ?? 7890)
    if (p !== proxyPort.value) proxyPort.value = p
    const n = String(np ?? '')
    if (n !== noProxy.value) noProxy.value = n
  },
)

async function onProxyFieldsSave() {
  const patch: Parameters<typeof api.updateSettings>[0] = {}
  const url = proxyUrl.value.trim()
  if (url && url !== store.settings.proxy_url) patch.proxy_url = url
  if (proxyPort.value && proxyPort.value !== store.settings.proxy_port) patch.proxy_port = proxyPort.value
  const np = noProxy.value.trim()
  if (np !== (store.settings.no_proxy ?? '')) patch.no_proxy = np
  if (Object.keys(patch).length > 0) await patchSettings(patch)
}

const sections = computed(() => [
  { key: 'environment', label: t('settings.environment.title') },
  { key: 'general', label: t('settings.general') },
  { key: 'shortcuts', label: t('settings.shortcuts.title') },
  { key: 'proxy', label: t('settings.proxy.title') },
  { key: 'update', label: t('settings.update.title') },
  { key: 'dataDir', label: t('settings.dataDir.title') },
])

const activeSection = ref('general')
const sectionEls = ref<Record<string, HTMLElement | null>>({})

function setSectionRef(key: string, el: unknown) {
  sectionEls.value[key] = (el as HTMLElement | null) ?? null
}

function scrollToSection(key: string) {
  activeSection.value = key
  sectionEls.value[key]?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}
</script>

<template>
  <div class="settings-layout">
    <!-- macOS System Settings Anchor Sider -->
    <aside class="settings-subnav">
      <button
        v-for="s in sections"
        :key="s.key"
        class="subnav-item"
        :class="{ active: activeSection === s.key }"
        @click="scrollToSection(s.key)"
      >
        <span class="subnav-label">{{ s.label }}</span>
      </button>
    </aside>

    <!-- Settings Content Area -->
    <div class="dl-page settings-content-area">
      <!-- Environment Section: which node/pnpm the launcher resolved, and a way
           back into the setup page once the environment is already green. -->
      <div :ref="(el: unknown) => setSectionRef('environment', el)" class="settings-section">
        <div class="section-title">
          <h3>{{ t('settings.environment.title') }}</h3>
        </div>
        <p class="section-sub">{{ t('settings.environment.desc') }}</p>

        <div class="apple-inset-group">
          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">Node.js</span>
              <span class="row-desc">
                {{ store.runtime?.node?.path ?? t('settings.environment.notFound') }}
              </span>
            </div>
            <a-tag :color="store.runtime?.node?.installed ? 'green' : 'red'">
              {{ store.runtime?.node?.version ?? t('settings.environment.missing') }}
            </a-tag>
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">pnpm</span>
              <span class="row-desc">
                {{ store.runtime?.pnpm?.path ?? t('settings.environment.notFound') }}
              </span>
            </div>
            <a-tag :color="store.runtime?.pnpm?.installed ? 'green' : 'red'">
              {{ store.runtime?.pnpm?.version ?? t('settings.environment.missing') }}
            </a-tag>
          </div>
        </div>

        <div class="env-actions">
          <button class="mac-secondary-btn" @click="openEnvironmentCheck">
            {{ t('settings.environment.openGuide') }}
          </button>
        </div>
      </div>

      <!-- General Section -->
      <div :ref="(el: unknown) => setSectionRef('general', el)" class="settings-section">
        <div class="section-title">
          <h3>{{ t('settings.general') }}</h3>
        </div>

        <div class="apple-inset-group">
          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.language') }}</span>
            </div>
            <a-select
              :model-value="store.settings.locale"
              style="width: 170px"
              size="small"
              @change="onLocaleChange"
            >
              <a-option v-for="l in SUPPORTED_LOCALES" :key="l.value" :value="l.value">
                {{ l.label }}
              </a-option>
            </a-select>
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.theme.label') }}</span>
            </div>
            <a-select
              :model-value="store.settings.theme"
              style="width: 170px"
              size="small"
              @change="onThemeChange"
            >
              <a-option v-for="o in THEME_OPTIONS" :key="o.value" :value="o.value">
                {{ o.label }}
              </a-option>
            </a-select>
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.minimizeToTray') }}</span>
            </div>
            <a-switch :model-value="store.settings.minimize_to_tray" size="small" @change="onTrayChange" />
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.autostart') }}</span>
            </div>
            <a-switch :model-value="store.settings.autostart" size="small" @change="onAutostartChange" />
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.logLevel.label') }}</span>
              <span class="row-desc">{{ t('settings.logLevel.hint') }}</span>
            </div>
            <a-select
              :model-value="store.settings.log_level"
              style="width: 170px"
              size="small"
              @change="onLogLevelChange"
            >
              <a-option v-for="o in LOG_LEVEL_OPTIONS" :key="o.value" :value="o.value">
                {{ o.label }}
              </a-option>
            </a-select>
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.terminal.label') }}</span>
              <span class="row-desc">{{ t('settings.terminal.hint') }}</span>
            </div>
            <a-select
              :model-value="store.settings.terminal"
              style="width: 170px"
              size="small"
              @change="onTerminalChange"
            >
              <a-option v-for="o in TERMINAL_OPTIONS" :key="o.value" :value="o.value">
                {{ o.label }}
              </a-option>
            </a-select>
          </div>
        </div>
      </div>

      <!-- Shortcuts Section -->
      <div :ref="(el: unknown) => setSectionRef('shortcuts', el)" class="settings-section">
        <div class="section-title">
          <h3>{{ t('settings.shortcuts.title') }}</h3>
          <p class="section-sub">{{ t('settings.shortcuts.desc') }}</p>
        </div>

        <div class="apple-inset-group">
          <div v-for="row in shortcutRows" :key="row.label" class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">
                {{ row.label }}
                <span v-if="row.native" class="native-tag-chip">{{ t('settings.shortcuts.nativeTag') }}</span>
              </span>
            </div>
            <div class="shortcut-keys-wrap">
              <span v-for="k in row.keys" :key="k" class="apple-kbd tnum">{{ k }}</span>
            </div>
          </div>
        </div>
        <p class="section-footnote">{{ t('settings.shortcuts.note') }}</p>
      </div>

      <!-- Proxy Section -->
      <div :ref="(el: unknown) => setSectionRef('proxy', el)" class="settings-section">
        <div class="section-title">
          <h3>{{ t('settings.proxy.title') }}</h3>
        </div>

        <div class="apple-inset-group">
          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.proxy.enabled') }}</span>
              <span class="row-desc">{{ t('settings.proxy.enabledHint') }}</span>
            </div>
            <a-switch :model-value="store.settings.proxy_enabled" size="small" @change="onProxyEnabledChange" />
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.proxy.url') }}</span>
            </div>
            <input
              v-model="proxyUrl"
              class="apple-input-sm"
              :disabled="!store.settings.proxy_enabled"
              placeholder="http://127.0.0.1"
              style="width: 220px"
              @blur="onProxyFieldsSave"
              @press-enter="onProxyFieldsSave"
            />
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.proxy.port') }}</span>
            </div>
            <input
              v-model="proxyPort"
              type="number"
              class="apple-input-sm"
              :disabled="!store.settings.proxy_enabled"
              min="1"
              max="65535"
              style="width: 110px"
              @blur="onProxyFieldsSave"
              @press-enter="onProxyFieldsSave"
            />
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.proxy.noProxy') }}</span>
              <span class="row-desc">{{ t('settings.proxy.noProxyHint') }}</span>
            </div>
            <input
              v-model="noProxy"
              class="apple-input-sm"
              :disabled="!store.settings.proxy_enabled"
              placeholder="127.0.0.1,localhost,::1"
              style="width: 220px"
              @blur="onProxyFieldsSave"
              @press-enter="onProxyFieldsSave"
            />
          </div>

          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.proxy.applyDsh') }}</span>
              <span class="row-desc">{{ t('settings.proxy.applyDshHint') }}</span>
            </div>
            <a-switch
              :model-value="store.settings.proxy_apply_dsh"
              :disabled="!store.settings.proxy_enabled"
              size="small"
              @change="onProxyApplyDshChange"
            />
          </div>
        </div>
      </div>

      <!-- Update Section -->
      <div :ref="(el: unknown) => setSectionRef('update', el)" class="settings-section">
        <div class="section-title">
          <h3>{{ t('settings.update.title') }}</h3>
        </div>

        <div class="apple-inset-group">
          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">
                DSH Launcher
                <span class="version-pill-badge tnum">v{{ launcherVersion }}</span>
              </span>
              <span class="row-desc">{{ t('settings.update.channelHint') }}</span>
            </div>
            <div class="update-controls">
              <a-select
                :model-value="updateChannel"
                style="width: 140px"
                size="small"
                @change="onUpdateChannelChange"
              >
                <a-option v-for="o in UPDATE_CHANNEL_OPTIONS" :key="o.value" :value="o.value">
                  {{ o.label }}
                </a-option>
              </a-select>
              <button class="mac-primary-btn" :disabled="checkingUpdate" @click="onCheckUpdate">
                {{ t('settings.update.check') }}
              </button>
            </div>
          </div>
        </div>

        <div v-if="updateInfo && !updateInfo.up_to_date" class="update-alert-card">
          <span class="update-msg">{{ t('settings.update.available', { version: updateInfo.latest }) }}</span>
          <button v-if="updateInfo.url" class="mac-secondary-btn" @click="api.openExternal(updateInfo.url!)">
            {{ t('settings.update.viewRelease') }}
          </button>
        </div>
      </div>

      <!-- Data Directory Section -->
      <div :ref="(el: unknown) => setSectionRef('dataDir', el)" class="settings-section">
        <div class="section-title">
          <h3>{{ t('settings.dataDir.title') }}</h3>
          <p class="section-sub">{{ t('settings.dataDir.desc') }}</p>
        </div>

        <div class="apple-inset-group">
          <div class="apple-inset-row">
            <div class="row-info">
              <span class="row-title">{{ t('settings.dataDir.pathLabel') }}</span>
              <span class="row-desc tnum">{{ dataDir || '—' }}</span>
            </div>
            <div class="dir-actions">
              <button class="mac-secondary-btn" @click="onOpenDataDir">
                {{ t('settings.dataDir.open') }}
              </button>
              <button class="mac-secondary-btn" @click="onOpenLauncherLog">
                {{ t('settings.viewLog') }}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.settings-layout {
  display: flex;
  min-height: 100%;
}

// Left Subnav
.settings-subnav {
  width: 160px;
  flex-shrink: 0;
  padding: 24px 12px;
  display: flex;
  flex-direction: column;
  gap: 3px;
  position: sticky;
  top: 0;
  height: calc(100vh - var(--dl-header-height));
  border-right: 1px solid var(--apple-separator);
}

.subnav-item {
  display: flex;
  align-items: center;
  width: 100%;
  padding: 7px 12px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--color-text-2);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  text-align: left;
  transition: all 0.15s ease;

  &:hover {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &.active {
    background: rgb(var(--primary-6) / 14%);
    color: rgb(var(--primary-6));
    font-weight: 600;
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

// Right Content
.settings-content-area {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.settings-section {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.section-title {
  h3 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    color: var(--color-text-1);
    letter-spacing: -0.01em;
  }

  .section-sub {
    margin: 3px 0 0;
    font-size: 12.5px;
    color: var(--color-text-3);
  }
}

.section-footnote {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--color-text-4);
}

.native-tag-chip {
  margin-left: 6px;
  padding: 1px 6px;
  font-size: 11px;
  border-radius: 5px;
  background: var(--apple-group-bg);
  color: var(--color-text-3);
  font-weight: normal;
}

.shortcut-keys-wrap {
  display: flex;
  align-items: center;
  gap: 4px;
}

.apple-kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: 22px;
  padding: 0 6px;
  font-size: 11.5px;
  font-weight: 600;
  border-radius: 5px;
  background: var(--apple-group-bg);
  border: 1px solid var(--apple-card-border);
  color: var(--color-text-1);
  box-shadow: 0 1px 1px rgba(0, 0, 0, 0.05);
}

.apple-input-sm {
  height: 30px;
  padding: 0 10px;
  font-size: 13px;
  border-radius: 7px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-group-bg);
  color: var(--color-text-1);
  outline: none;
  transition: all 0.16s ease;

  &:focus {
    background: var(--apple-card-bg);
    border-color: rgb(var(--primary-6));
    box-shadow: 0 0 0 2px rgb(var(--primary-6) / 18%);
  }

  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
}

.mac-primary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 12px;
  font-size: 12.5px;
  font-weight: 500;
  border-radius: 7px;
  border: none;
  background: rgb(var(--primary-6));
  color: #fff;
  cursor: pointer;
  transition: all 0.16s ease;

  &:hover:not(:disabled) {
    filter: brightness(1.06);
  }

  &:active:not(:disabled) {
    transform: scale(var(--apple-active-scale));
  }
}

.mac-secondary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 10px;
  font-size: 12.5px;
  font-weight: 500;
  border-radius: 7px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  cursor: pointer;
  transition: all 0.16s ease;

  &:hover {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.version-pill-badge {
  margin-left: 6px;
  padding: 1px 7px;
  font-size: 11.5px;
  font-weight: 600;
  border-radius: 6px;
  background: var(--apple-group-bg);
  color: var(--color-text-2);
}

.update-controls {
  display: flex;
  align-items: center;
  gap: 8px;
}

.update-alert-card {
  margin-top: 10px;
  padding: 10px 14px;
  border-radius: 9px;
  background: rgb(var(--primary-6) / 10%);
  border: 1px solid rgb(var(--primary-6) / 20%);
  display: flex;
  align-items: center;
  justify-content: space-between;

  .update-msg {
    font-size: 13px;
    color: rgb(var(--primary-6));
    font-weight: 500;
  }
}

.dir-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}
</style>

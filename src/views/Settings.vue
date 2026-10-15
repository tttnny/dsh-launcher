<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { Host, LauncherUpdateInfo, LogLevel, ThemeMode } from '@/api/types'
import { useAction } from '@/composables/useAction'
import { SUPPORTED_LOCALES } from '@/i18n'
import { useLauncherStore } from '@/stores/launcher'
import { SHORTCUT_DOCS } from '@/shortcuts'
import DlIcon from '@/components/DlIcon.vue'

// Settings is a single scroll of inset groups. The section rail on the left is
// a table of contents, not a second navigation — the app's real navigation is
// the sidebar, and duplicating it here was the old page's biggest confusion.

const router = useRouter()
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

const TERMINAL_OPTIONS = computed(() => [
  { value: 'system', label: t('settings.terminal.system') },
  { value: 'ghostty', label: t('settings.terminal.ghostty') },
])

const UPDATE_CHANNEL_OPTIONS = computed(() => [
  { value: 'dev', label: t('settings.update.channel.dev') },
  { value: 'release', label: t('settings.update.channel.release') },
])

const shortcutRows = computed(() =>
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

// --- Environment --------------------------------------------------------------

const envOk = computed(() => !!store.runtime?.node?.installed && !!store.runtime?.pnpm?.installed)

// --- Hosts ----------------------------------------------------------------------

/** The machines the launcher manages; 本机 is the built-in one (ADR-0006). */
const hosts = ref<Host[]>([])

// --- Update -------------------------------------------------------------------

const launcherVersion = ref('')
const updateInfo = ref<LauncherUpdateInfo | null>(null)
const updateChannel = ref<'dev' | 'release'>('dev')
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
  try {
    hosts.value = await api.listHosts()
  } catch {
    hosts.value = []
  }
})

const checkUpdateAction = useAction(() => api.checkLauncherUpdate(updateChannel.value))

async function onCheckUpdate() {
  const res = await checkUpdateAction.run()
  if (res === undefined) return
  updateInfo.value = res
  if (res.up_to_date) Message.success(t('settings.update.upToDate'))
}

// --- Proxy fields -------------------------------------------------------------

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

async function saveProxyFields() {
  const patch: Parameters<typeof api.updateSettings>[0] = {}
  const url = proxyUrl.value.trim()
  if (url && url !== store.settings.proxy_url) patch.proxy_url = url
  if (proxyPort.value && proxyPort.value !== store.settings.proxy_port) patch.proxy_port = proxyPort.value
  const np = noProxy.value.trim()
  if (np !== (store.settings.no_proxy ?? '')) patch.no_proxy = np
  if (Object.keys(patch).length > 0) await patchSettings(patch)
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

// --- Section rail -------------------------------------------------------------

const sections = computed(() => [
  { key: 'general', label: t('settings.general') },
  { key: 'hosts', label: t('settings.hosts.title') },
  { key: 'environment', label: t('settings.environment.title') },
  { key: 'proxy', label: t('settings.proxy.title') },
  { key: 'update', label: t('settings.update.title') },
  { key: 'data', label: t('settings.dataDir.title') },
  { key: 'shortcuts', label: t('settings.shortcuts.title') },
])

const sectionEls = ref<Record<string, HTMLElement | null>>({})
const activeSection = ref('general')
const activeScrollBox = ref<HTMLElement | null>(null)
/** Timestamp until which the scroll spy ignores geometry (rail clicks). */
let spyLockedUntil = 0

function setSectionRef(key: string, el: unknown) {
  sectionEls.value[key] = (el as HTMLElement | null) ?? null
}

function scrollTo(key: string) {
  activeSection.value = key
  // The spy is paused while the smooth scroll is in flight: it recomputes from
  // geometry on every scroll frame and would otherwise snap the highlight back
  // to whatever section happens to be under the reading line mid-flight.
  spyLockedUntil = Date.now() + 700
  sectionEls.value[key]?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

// Scroll spy: the rail is a table of contents, so it has to say which section
// the viewport is actually in. The scroller lives in App.vue (`.dl-content`),
// so it is resolved from a section element once mounted.
function scroller(): HTMLElement | null {
  return (sectionEls.value.general?.closest('.dl-content') as HTMLElement | null) ?? null
}

function trackActiveSection() {
  const box = scroller()
  if (!box) return
  if (Date.now() < spyLockedUntil) return
  const top = box.getBoundingClientRect().top

  // The last section can never clear the reading line: the scroller runs out of
  // room before its heading gets there. At the bottom, the bottom section is
  // what the reader is looking at.
  if (box.scrollTop + box.clientHeight >= box.scrollHeight - 4) {
    activeSection.value = sections.value[sections.value.length - 1]?.key ?? activeSection.value
    return
  }

  let current = activeSection.value
  for (const s of sections.value) {
    const el = sectionEls.value[s.key]
    // 72px is the reading line: a section counts as current once its label has
    // passed under the top of the viewport.
    if (el && el.getBoundingClientRect().top - top <= 72) current = s.key
  }
  activeSection.value = current
}

onMounted(() => {
  const box = scroller()
  if (!box) return
  activeScrollBox.value = box
  box.addEventListener('scroll', trackActiveSection, { passive: true })
  trackActiveSection()
})

onUnmounted(() => {
  activeScrollBox.value?.removeEventListener('scroll', trackActiveSection)
})
</script>

<template>
  <div class="dl-page settings-page">
    <header class="dl-head">
      <div class="dl-head-text">
        <h1>{{ t('settings.title') }}</h1>
        <p class="dl-head-sub">{{ t('settings.subtitle') }}</p>
      </div>
      <div class="dl-head-actions">
        <span class="dl-chip">DSH Launcher {{ launcherVersion ? `v${launcherVersion}` : '' }}</span>
      </div>
    </header>

    <div class="settings-layout">
      <nav class="settings-rail">
        <button
          v-for="s in sections"
          :key="s.key"
          class="rail-item"
          :class="{ 'is-active': activeSection === s.key }"
          :aria-current="activeSection === s.key ? 'true' : undefined"
          @click="scrollTo(s.key)"
        >
          {{ s.label }}
        </button>
      </nav>

      <div class="settings-body">
        <!-- General -->
        <section :ref="(el) => setSectionRef('general', el)" class="dl-section">
          <div class="dl-section-label">{{ t('settings.general') }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <span class="dl-row-label">{{ t('settings.language') }}</span>
              <a-select
                :model-value="store.settings.locale"
                style="width: 190px"
                size="small"
                @change="(v) => patchSettings({ locale: String(v) })"
              >
                <a-option v-for="l in SUPPORTED_LOCALES" :key="l.value" :value="l.value">{{ l.label }}</a-option>
              </a-select>
            </div>
            <div class="dl-row">
              <span class="dl-row-label">{{ t('settings.theme.label') }}</span>
              <a-select
                :model-value="store.settings.theme"
                style="width: 190px"
                size="small"
                @change="(v) => patchSettings({ theme: String(v) as ThemeMode })"
              >
                <a-option v-for="o in THEME_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</a-option>
              </a-select>
            </div>
            <div class="dl-row">
              <span class="dl-row-label">{{ t('settings.minimizeToTray') }}</span>
              <a-switch
                :model-value="store.settings.minimize_to_tray"
                size="small"
                @change="(v) => patchSettings({ minimize_to_tray: Boolean(v) })"
              />
            </div>
            <div class="dl-row">
              <span class="dl-row-label">{{ t('settings.autostart') }}</span>
              <a-switch
                :model-value="store.settings.autostart"
                size="small"
                @change="(v) => patchSettings({ autostart: Boolean(v) })"
              />
            </div>
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('settings.logLevel.label') }}</span>
                <span class="dl-row-desc">{{ t('settings.logLevel.hint') }}</span>
              </div>
              <a-select
                :model-value="store.settings.log_level"
                style="width: 190px"
                size="small"
                @change="(v) => patchSettings({ log_level: String(v) as LogLevel })"
              >
                <a-option v-for="o in LOG_LEVEL_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</a-option>
              </a-select>
            </div>
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('settings.terminal.label') }}</span>
                <span class="dl-row-desc">{{ t('settings.terminal.hint') }}</span>
              </div>
              <a-select
                :model-value="store.settings.terminal"
                style="width: 220px"
                size="small"
                @change="(v) => patchSettings({ terminal: String(v) })"
              >
                <a-option v-for="o in TERMINAL_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</a-option>
              </a-select>
            </div>
          </div>
        </section>

        <!-- Hosts: the machines the launcher manages. 本机 is built in and has
             no delete control — the record cannot be removed. -->
        <section :ref="(el) => setSectionRef('hosts', el)" class="dl-section">
          <div class="dl-section-label">
            {{ t('settings.hosts.title') }}
            <span class="dl-section-count">{{ hosts.length }}</span>
          </div>
          <div class="dl-surface">
            <div v-for="h in hosts" :key="h.id" class="dl-row">
              <span class="dl-row-label">
                {{ h.display_name }}
                <span v-if="h.builtin" class="dl-tag dl-tag--muted">{{ t('settings.hosts.builtin') }}</span>
              </span>
              <span v-if="h.ssh_target" class="dl-row-desc dl-mono">{{ h.ssh_target }}</span>
            </div>
          </div>
          <p class="section-note">{{ t('settings.hosts.note') }}</p>
        </section>

        <!-- Environment: a compact readout that links to the full check page. -->
        <section :ref="(el) => setSectionRef('environment', el)" class="dl-section">
          <div class="dl-section-label">
            {{ t('settings.environment.title') }}
            <span class="dl-section-count">
              <span class="dl-tag" :class="envOk ? 'dl-tag--run' : 'dl-tag--danger'">
                {{ envOk ? t('nav.envReady') : t('nav.envMissing') }}
              </span>
            </span>
          </div>
          <div class="dl-surface">
            <div class="dl-row">
              <span class="dl-row-label">Node.js</span>
              <span class="row-path dl-mono" :title="store.runtime?.node?.path ?? ''">
                {{ store.runtime?.node?.path ?? t('settings.environment.notFound') }}
              </span>
              <span class="dl-chip">{{ store.runtime?.node?.version ?? t('settings.environment.missing') }}</span>
            </div>
            <div class="dl-row">
              <span class="dl-row-label">pnpm</span>
              <span class="row-path dl-mono" :title="store.runtime?.pnpm?.path ?? ''">
                {{ store.runtime?.pnpm?.path ?? t('settings.environment.notFound') }}
              </span>
              <span class="dl-chip">{{ store.runtime?.pnpm?.version ?? t('settings.environment.missing') }}</span>
            </div>
            <div class="dl-row">
              <span class="dl-row-desc">{{ t('settings.environment.desc') }}</span>
              <button class="dl-btn dl-btn--quiet" @click="router.push({ name: 'setup' })">
                {{ t('settings.environment.openGuide') }}
              </button>
            </div>
          </div>
        </section>

        <!-- Proxy -->
        <section :ref="(el) => setSectionRef('proxy', el)" class="dl-section">
          <div class="dl-section-label">{{ t('settings.proxy.title') }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('settings.proxy.enabled') }}</span>
                <span class="dl-row-desc">{{ t('settings.proxy.enabledHint') }}</span>
              </div>
              <a-switch
                :model-value="store.settings.proxy_enabled"
                size="small"
                @change="(v) => patchSettings({ proxy_enabled: Boolean(v) })"
              />
            </div>
            <div class="dl-row">
              <span class="dl-row-label">{{ t('settings.proxy.url') }}</span>
              <input
                v-model="proxyUrl"
                class="dl-input dl-input--mono"
                :disabled="!store.settings.proxy_enabled"
                placeholder="http://127.0.0.1"
                style="width: 240px"
                @blur="saveProxyFields"
                @keydown.enter="saveProxyFields"
              />
            </div>
            <div class="dl-row">
              <span class="dl-row-label">{{ t('settings.proxy.port') }}</span>
              <input
                v-model="proxyPort"
                type="number"
                class="dl-input dl-input--mono"
                :disabled="!store.settings.proxy_enabled"
                min="1"
                max="65535"
                style="width: 120px"
                @blur="saveProxyFields"
                @keydown.enter="saveProxyFields"
              />
            </div>
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('settings.proxy.noProxy') }}</span>
                <span class="dl-row-desc">{{ t('settings.proxy.noProxyHint') }}</span>
              </div>
              <input
                v-model="noProxy"
                class="dl-input dl-input--mono"
                :disabled="!store.settings.proxy_enabled"
                placeholder="127.0.0.1,localhost,::1"
                style="width: 240px"
                @blur="saveProxyFields"
                @keydown.enter="saveProxyFields"
              />
            </div>
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('settings.proxy.applyDsh') }}</span>
                <span class="dl-row-desc">{{ t('settings.proxy.applyDshHint') }}</span>
              </div>
              <a-switch
                :model-value="store.settings.proxy_apply_dsh"
                :disabled="!store.settings.proxy_enabled"
                size="small"
                @change="(v) => patchSettings({ proxy_apply_dsh: Boolean(v) })"
              />
            </div>
          </div>
        </section>

        <!-- Update -->
        <section :ref="(el) => setSectionRef('update', el)" class="dl-section">
          <div class="dl-section-label">{{ t('settings.update.title') }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('settings.update.channelHint') }}</span>
              </div>
              <div class="row-actions">
                <a-select
                  :model-value="updateChannel"
                  style="width: 150px"
                  size="small"
                  @change="(v) => { updateChannel = String(v) === 'release' ? 'release' : 'dev'; updateInfo = null }"
                >
                  <a-option v-for="o in UPDATE_CHANNEL_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</a-option>
                </a-select>
                <button class="dl-btn dl-btn--quiet" :disabled="checkUpdateAction.busy['*']" @click="onCheckUpdate">
                  {{ t('settings.update.check') }}
                </button>
              </div>
            </div>
            <div v-if="updateInfo && !updateInfo.up_to_date" class="dl-row">
              <span class="dl-row-label">{{ t('settings.update.available', { version: updateInfo.latest }) }}</span>
              <button v-if="updateInfo.url" class="dl-btn dl-btn--primary" @click="api.openExternal(updateInfo.url!)">
                {{ t('settings.update.viewRelease') }}
              </button>
            </div>
          </div>
        </section>

        <!-- Data directory -->
        <section :ref="(el) => setSectionRef('data', el)" class="dl-section">
          <div class="dl-section-label">{{ t('settings.dataDir.title') }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('settings.dataDir.pathLabel') }}</span>
                <span class="row-desc dl-mono">{{ dataDir || '—' }}</span>
              </div>
              <div class="row-actions">
                <button class="dl-btn dl-btn--quiet" @click="onOpenDataDir">
                  <DlIcon name="folder" :size="13" />
                  {{ t('settings.dataDir.open') }}
                </button>
                <button class="dl-btn dl-btn--quiet" @click="openLogAction.run()">
                  <DlIcon name="log" :size="13" />
                  {{ t('settings.dataDir.viewLog') }}
                </button>
              </div>
            </div>
            <div class="dl-row">
              <span class="dl-row-desc">{{ t('settings.dataDir.desc') }}</span>
            </div>
          </div>
        </section>

        <!-- Shortcuts -->
        <section :ref="(el) => setSectionRef('shortcuts', el)" class="dl-section">
          <div class="dl-section-label">{{ t('settings.shortcuts.title') }}</div>
          <div class="dl-surface">
            <div v-for="row in shortcutRows" :key="row.label" class="dl-row">
              <span class="dl-row-label">
                {{ row.label }}
                <span v-if="row.native" class="dl-tag dl-tag--muted">{{ t('settings.shortcuts.nativeTag') }}</span>
              </span>
              <span class="kbd-row">
                <span v-for="k in row.keys" :key="k" class="dl-kbd">{{ k }}</span>
              </span>
            </div>
          </div>
          <p class="section-note">{{ t('settings.shortcuts.note') }}</p>
        </section>
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.settings-layout {
  display: grid;
  grid-template-columns: 150px minmax(0, 1fr);
  gap: 28px;
  align-items: start;
}

.settings-rail {
  position: sticky;
  top: 20px;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.rail-item {
  padding: 5px 9px;
  border: none;
  border-radius: var(--dl-r-ctl);
  background: transparent;
  color: var(--dl-ink-3);
  font-family: inherit;
  font-size: var(--dl-text-sm);
  font-weight: 550;
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--dl-fast) var(--dl-ease),
    color var(--dl-fast) var(--dl-ease);

  &:hover {
    background: var(--dl-surface-2);
    color: var(--dl-ink);
  }

  // The current section is stated with ink weight plus a signal tick, not with
  // a filled pill: colour in this app means state, and "where am I" is state.
  &.is-active {
    position: relative;
    background: var(--dl-surface-2);
    color: var(--dl-ink);
    font-weight: 650;

    &::before {
      content: '';
      position: absolute;
      top: 50%;
      left: -6px;
      width: 2px;
      height: 14px;
      transform: translateY(-50%);
      border-radius: 1px;
      background: var(--dl-signal);
    }
  }
}

.settings-body {
  min-width: 0;
}

.row-path {
  flex: 1;
  min-width: 0;
  margin: 0 12px;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  text-align: right;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.kbd-row {
  display: flex;
  align-items: center;
  gap: 4px;
}

.dl-kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: 22px;
  padding: 0 6px;
  border: 1px solid var(--dl-line-strong);
  border-bottom-width: 2px;
  border-radius: 5px;
  background: var(--dl-surface-2);
  color: var(--dl-ink-2);
  font-family: var(--dl-font-mono);
  font-size: var(--dl-text-xs);
  font-weight: 600;
}

.section-note {
  margin: 8px 2px 0;
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
}

@media (max-width: 900px) {
  .settings-layout {
    grid-template-columns: 1fr;
  }

  .settings-rail {
    position: static;
    flex-direction: row;
    flex-wrap: wrap;
  }

  // Horizontal rail: a left tick would sit outside the button and clip.
  .rail-item.is-active::before {
    display: none;
  }
}
</style>

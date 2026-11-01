<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { LOCAL_HOST_ID } from '@/api/types'
import type {
  ConnectionTest,
  HostInput,
  HostStatus,
  HostWithStatus,
  LauncherUpdateInfo,
  LogLevel,
  RemoteTool,
  RuntimeStatus,
  SshAlias,
  ThemeMode,
} from '@/api/types'
import { useAction } from '@/composables/useAction'
import { useCopy } from '@/composables/useCopy'
import { useLauncherStore } from '@/stores/launcher'
import { SHORTCUT_DOCS } from '@/shortcuts'
import DlIcon from '@/components/DlIcon.vue'

// Settings is a single scroll of inset groups. The section rail on the left is
// a table of contents, not a second navigation — the app's real navigation is
// the sidebar, and duplicating it here was the old page's biggest confusion.

const router = useRouter()
const store = useLauncherStore()

const THEME_OPTIONS = computed<{ value: ThemeMode; label: string }[]>(() => [
  { value: 'light', label: `浅色` },
  { value: 'dark', label: `深色` },
  { value: 'system', label: `跟随系统` },
])

const LOG_LEVEL_OPTIONS = computed<{ value: LogLevel; label: string }[]>(() => [
  { value: 'debug', label: `调试 (debug)` },
  { value: 'info', label: `信息 (info)` },
  { value: 'warn', label: `警告 (warn)` },
  { value: 'error', label: `错误 (error)` },
])

const TERMINAL_OPTIONS = computed(() => [
  { value: 'system', label: `系统终端（Terminal.app）` },
  { value: 'ghostty', label: `Ghostty` },
])

const UPDATE_CHANNEL_OPTIONS = computed(() => [
  { value: 'dev', label: `dev 渠道` },
  { value: 'release', label: `release 渠道` },
])

const shortcutRows = computed(() =>
  SHORTCUT_DOCS.map((d) => ({
    label: d.label,
    keys: d.keys,
    native: d.native ?? false,
  })),
)

const saveAction = useAction(
  (patch: Parameters<typeof api.updateSettings>[0]) =>
    api.updateSettings(patch),
  { success: () => `设置已保存` },
)

async function patchSettings(patch: Parameters<typeof api.updateSettings>[0]) {
  const res = await saveAction.run(patch)
  if (res === undefined) return
  store.settings = res
}

// --- Update -------------------------------------------------------------------

const updateInfo = ref<LauncherUpdateInfo | null>(null)
const updateChannel = ref<'dev' | 'release'>('dev')
const dataDir = ref('')

onMounted(async () => {
  try {
    dataDir.value = await api.getLauncherDirectory()
  } catch {
    dataDir.value = ''
  }
})

const checkUpdateAction = useAction(() =>
  api.checkLauncherUpdate(updateChannel.value),
)

async function onCheckUpdate() {
  const res = await checkUpdateAction.run()
  if (res === undefined) return
  updateInfo.value = res
  if (res.up_to_date) Message.success(`当前已是最新版本`)
}

// --- Proxy fields -------------------------------------------------------------

const proxyUrl = ref(store.settings.proxy_url ?? '')
const proxyPort = ref(store.settings.proxy_port ?? 7890)
const noProxy = ref(store.settings.no_proxy ?? '')

watch(
  () =>
    [
      store.settings.proxy_url,
      store.settings.proxy_port,
      store.settings.no_proxy,
    ] as const,
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
  if (proxyPort.value && proxyPort.value !== store.settings.proxy_port)
    patch.proxy_port = proxyPort.value
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
  success: (path) => `已在访达中定位日志：${path}`,
})

// --- Section rail -------------------------------------------------------------

const sections = computed(() => [
  { key: 'general', label: `常规` },
  { key: 'proxy', label: `代理` },
  { key: 'update', label: `启动器更新` },
  { key: 'data', label: `数据目录` },
  { key: 'shortcuts', label: `键盘快捷键` },
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
  return (
    (sectionEls.value.general?.closest('.dl-content') as HTMLElement | null) ??
    null
  )
}

function trackActiveSection() {
  const box = scroller()
  if (!box) return
  if (Date.now() < spyLockedUntil) return
  const top = box.getBoundingClientRect().top

  // The last section can never clear the reading line: the scroller runs out of
  // room before its heading gets there. When the page is truly at the bottom, the
  // last section is what the reader is looking at, so it is selected here while
  // the reading line keeps handling the sections in between.
  if (box.scrollTop + box.clientHeight >= box.scrollHeight - 4) {
    activeSection.value =
      sections.value[sections.value.length - 1]?.key ?? activeSection.value
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
        <h1>{{ `设置` }}</h1>
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
          <div class="dl-section-label">{{ `常规` }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <span id="set-theme" class="dl-row-label">{{ `主题` }}</span>
              <a-select
                :model-value="store.settings.theme"
                aria-labelledby="set-theme"
                style="width: 190px"
                size="small"
                @change="
                  (v) => patchSettings({ theme: String(v) as ThemeMode })
                "
              >
                <a-option
                  v-for="o in THEME_OPTIONS"
                  :key="o.value"
                  :value="o.value"
                  >{{ o.label }}</a-option
                >
              </a-select>
            </div>
            <div class="dl-row">
              <span id="set-minimize" class="dl-row-label">{{
                `关闭主窗口时最小化到系统托盘`
              }}</span>
              <a-switch
                :model-value="store.settings.minimize_to_tray"
                aria-labelledby="set-minimize"
                class="row-switch"
                size="small"
                @change="(v) => patchSettings({ minimize_to_tray: Boolean(v) })"
              />
            </div>
            <div class="dl-row">
              <span id="set-autostart" class="dl-row-label">{{
                `开机自动启动启动器`
              }}</span>
              <a-switch
                :model-value="store.settings.autostart"
                aria-labelledby="set-autostart"
                class="row-switch"
                size="small"
                @change="(v) => patchSettings({ autostart: Boolean(v) })"
              />
            </div>
            <div class="dl-row">
              <span id="set-log-level" class="dl-row-label">{{
                `日志等级`
              }}</span>
              <a-select
                :model-value="store.settings.log_level"
                aria-labelledby="set-log-level"
                style="width: 190px"
                size="small"
                @change="
                  (v) => patchSettings({ log_level: String(v) as LogLevel })
                "
              >
                <a-option
                  v-for="o in LOG_LEVEL_OPTIONS"
                  :key="o.value"
                  :value="o.value"
                  >{{ o.label }}</a-option
                >
              </a-select>
            </div>
            <div class="dl-row">
              <span id="set-terminal" class="dl-row-label">{{
                `外部终端`
              }}</span>
              <a-select
                :model-value="store.settings.terminal"
                aria-labelledby="set-terminal"
                style="width: 220px"
                size="small"
                @change="(v) => patchSettings({ terminal: String(v) })"
              >
                <a-option
                  v-for="o in TERMINAL_OPTIONS"
                  :key="o.value"
                  :value="o.value"
                  >{{ o.label }}</a-option
                >
              </a-select>
            </div>
          </div>
        </section>

        <!-- Proxy -->
        <section :ref="(el) => setSectionRef('proxy', el)" class="dl-section">
          <div class="dl-section-label">{{ `代理` }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <span id="set-proxy-enabled" class="dl-row-label">{{
                `启用代理`
              }}</span>
              <a-switch
                :model-value="store.settings.proxy_enabled"
                aria-labelledby="set-proxy-enabled"
                class="row-switch"
                size="small"
                @change="(v) => patchSettings({ proxy_enabled: Boolean(v) })"
              />
            </div>
            <div class="dl-row">
              <span id="set-proxy-url" class="dl-row-label">{{
                `代理地址`
              }}</span>
              <input
                v-model="proxyUrl"
                aria-labelledby="set-proxy-url"
                class="dl-input dl-input--mono"
                :disabled="!store.settings.proxy_enabled"
                placeholder="http://127.0.0.1"
                style="width: 240px"
                @blur="saveProxyFields"
                @keydown.enter="saveProxyFields"
              />
            </div>
            <div class="dl-row">
              <span id="set-proxy-port" class="dl-row-label">{{ `端口` }}</span>
              <input
                v-model="proxyPort"
                aria-labelledby="set-proxy-port"
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
                <span id="set-no-proxy" class="dl-row-label">{{
                  `绕过列表`
                }}</span>
                <span class="dl-row-desc">{{
                  `不走代理的地址，逗号分隔。`
                }}</span>
              </div>
              <input
                v-model="noProxy"
                aria-labelledby="set-no-proxy"
                class="dl-input dl-input--mono"
                :disabled="!store.settings.proxy_enabled"
                placeholder="127.0.0.1,localhost,::1"
                style="width: 240px"
                @blur="saveProxyFields"
                @keydown.enter="saveProxyFields"
              />
            </div>
            <div class="dl-row">
              <span id="set-proxy-apply" class="dl-row-label">{{
                `在 DSH 中生效`
              }}</span>
              <a-switch
                :model-value="store.settings.proxy_apply_dsh"
                aria-labelledby="set-proxy-apply"
                :disabled="!store.settings.proxy_enabled"
                class="row-switch"
                size="small"
                @change="(v) => patchSettings({ proxy_apply_dsh: Boolean(v) })"
              />
            </div>
          </div>
        </section>

        <!-- Update -->
        <section :ref="(el) => setSectionRef('update', el)" class="dl-section">
          <div class="dl-section-label">{{ `启动器更新` }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <span class="dl-row-label">{{ `更新渠道` }}</span>
              <div class="row-actions">
                <a-select
                  :model-value="updateChannel"
                  style="width: 150px"
                  size="small"
                  @change="
                    (v) => {
                      updateChannel =
                        String(v) === 'release' ? 'release' : 'dev'
                      updateInfo = null
                    }
                  "
                >
                  <a-option
                    v-for="o in UPDATE_CHANNEL_OPTIONS"
                    :key="o.value"
                    :value="o.value"
                    >{{ o.label }}</a-option
                  >
                </a-select>
                <button
                  class="dl-btn dl-btn--quiet"
                  :disabled="checkUpdateAction.busy['*']"
                  @click="onCheckUpdate"
                >
                  {{ `检查更新` }}
                </button>
              </div>
            </div>
            <div v-if="updateInfo && !updateInfo.up_to_date" class="dl-row">
              <span class="dl-row-label">{{
                `发现新版本 v${updateInfo.latest}`
              }}</span>
              <button
                v-if="updateInfo.url"
                class="dl-btn dl-btn--primary"
                @click="api.openExternal(updateInfo.url!)"
              >
                {{ `前往下载` }}
              </button>
            </div>
          </div>
        </section>

        <!-- Data directory -->
        <section :ref="(el) => setSectionRef('data', el)" class="dl-section">
          <div class="dl-section-label">{{ `数据目录` }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ `目录路径` }}</span>
                <span class="row-desc dl-mono">{{ dataDir || '—' }}</span>
              </div>
              <div class="row-actions">
                <button class="dl-btn dl-btn--quiet" @click="onOpenDataDir">
                  <DlIcon name="folder" :size="13" />
                  {{ `打开目录` }}
                </button>
                <button
                  class="dl-btn dl-btn--quiet"
                  @click="openLogAction.run()"
                >
                  <DlIcon name="log" :size="13" />
                  {{ `查看日志` }}
                </button>
              </div>
            </div>
          </div>
        </section>

        <!-- Shortcuts -->
        <section
          :ref="(el) => setSectionRef('shortcuts', el)"
          class="dl-section"
        >
          <div class="dl-section-label">
            {{ `键盘快捷键` }}
          </div>
          <div class="dl-surface">
            <div v-for="row in shortcutRows" :key="row.label" class="dl-row">
              <span class="dl-row-label">
                {{ row.label }}
                <span v-if="row.native" class="dl-tag dl-tag--muted">{{
                  `原生`
                }}</span>
              </span>
              <span class="kbd-row">
                <span v-for="k in row.keys" :key="k" class="dl-kbd">{{
                  k
                }}</span>
              </span>
            </div>
          </div>
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

.row-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

// Keycaps share one column: the fixed width makes a two-key combo and a single
// key begin at the same left edge.
.kbd-row {
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 4px;
  min-width: 72px;
}

// Enlarge the small switch's hit area; the visual size stays compact.
.row-switch {
  min-width: 34px !important;
  height: 20px !important;

  :deep(.arco-switch-handle) {
    top: 3px;
    left: 3px;
    width: 14px;
    height: 14px;
  }

  &.arco-switch-checked :deep(.arco-switch-handle) {
    left: calc(100% - 17px);
  }
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

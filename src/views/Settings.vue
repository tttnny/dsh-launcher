<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
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
import { SUPPORTED_LOCALES } from '@/i18n'
import { useLauncherStore } from '@/stores/launcher'
import { directoryCommand } from '@/remoteCommand'
import { SHORTCUT_DOCS } from '@/shortcuts'
import DlIcon from '@/components/DlIcon.vue'

// Settings is a single scroll of inset groups. The section rail on the left is
// a table of contents, not a second navigation — the app's real navigation is
// the sidebar, and duplicating it here was the old page's biggest confusion.

const router = useRouter()
const { t, locale } = useI18n()
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

// The readout answers for one host: the toolchain is per host, because a path is
// a fact about one machine's filesystem (ADR-0005 追加). The host is the
// launcher's one 当前主机 (ADR-0006 §3) — the same state the Library's picker and
// the board's filter read — so the section reports whichever machine those pages
// are showing, and «切换主机» here moves them too.
const envHostId = computed({
  get: () => store.currentHostId,
  set: (id: string) => void store.selectHost(id),
})
// 本机's answer is already in the store; a remote host starts empty, so its row
// never wears the local machine's paths while its own answer is on the way.
const envRuntime = ref<RuntimeStatus | null>(
  store.currentHostId === LOCAL_HOST_ID ? store.runtime : null,
)
/** Why this host's paths cannot be shown (an unreachable machine, a bad target). */
const envError = ref('')

const envOk = computed(
  () => !!envRuntime.value?.node?.installed && !!envRuntime.value?.pnpm?.installed,
)

async function loadEnv() {
  const wanted = envHostId.value
  try {
    const answer = await api.getRuntimeStatus(false, wanted)
    if (wanted !== envHostId.value) return
    envError.value = ''
    envRuntime.value = answer
  } catch (e) {
    if (wanted !== envHostId.value) return
    // A machine that could not be reached has no conclusions to report. Leaving
    // the row to say 「找不到」 would claim that machine has no toolchain.
    envError.value = String(e)
    envRuntime.value = null
  }
}

watch(envHostId, () => {
  envRuntime.value = null
  envError.value = ''
  void loadEnv()
})

// --- Hosts ----------------------------------------------------------------------

/**
 * The machines the launcher manages, each with the status last observed for it.
 * 本机 is the built-in one: no status badge, no test button, no delete — there
 * is nothing to dial (ADR-0006).
 */
const hosts = ref<HostWithStatus[]>([])
const sshAliases = ref<SshAlias[]>([])
/** The host a connection test is in flight for. */
const testingHostId = ref('')
/** The last connection-test result, with the host it belongs to. */
const testResult = ref<{ hostId: string; test: ConnectionTest } | null>(null)

const { copy } = useCopy()

async function loadHosts() {
  try {
    hosts.value = await api.listHosts()
    // The same list is what the Library's host picker and the board's filter
    // read: a host added, renamed or deleted here has to reach those pages, and
    // so does the fallback when the current host is the one that went away.
    store.setHosts(hosts.value)
  } catch {
    hosts.value = []
  }
}

/** The add/edit sheet's fields; `extra_env` is a textarea because that is how users think of it. */
interface HostForm {
  display_name: string
  ssh_target: string
  registry: string
  data_dir: string
  extra_env: string
}

function emptyHostForm(): HostForm {
  return { display_name: '', ssh_target: '', registry: '', data_dir: '', extra_env: '' }
}

const hostSheet = ref(false)
const hostEditingId = ref('')
const hostForm = ref<HostForm>(emptyHostForm())

const saveHostAction = useAction(
  (id: string, input: HostInput) => (id ? api.updateHost(id, input) : api.createHost(input)),
  { success: () => t('settings.hosts.saved') },
)

/** `KEY=VALUE` lines → the map the backend takes; `null` when a line is unusable. */
function parseEnvLines(text: string): Record<string, string> | null {
  const env: Record<string, string> = {}
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line) continue
    const at = line.indexOf('=')
    if (at <= 0) return null
    env[line.slice(0, at).trim()] = line.slice(at + 1).trim()
  }
  return env
}

function startAddHost() {
  hostEditingId.value = ''
  hostForm.value = emptyHostForm()
  hostSheet.value = true
  void loadAliases()
}

function startEditHost(host: HostWithStatus) {
  hostEditingId.value = host.id
  hostForm.value = {
    display_name: host.display_name,
    ssh_target: host.ssh_target,
    registry: host.registry ?? '',
    data_dir: host.data_dir ?? '',
    extra_env: Object.entries(host.extra_env ?? {})
      .map(([key, value]) => `${key}=${value}`)
      .join('\n'),
  }
  hostSheet.value = true
  void loadAliases()
}

async function loadAliases() {
  if (sshAliases.value.length) return
  try {
    sshAliases.value = await api.listSshAliases()
  } catch {
    sshAliases.value = []
  }
}

/** The alias as the row shows it: what it points at, when the config says. */
function aliasTarget(alias: SshAlias): string {
  const host = alias.user
    ? `${alias.user}@${alias.host_name ?? alias.alias}`
    : (alias.host_name ?? '')
  return [host, alias.port ? `:${alias.port}` : ''].filter(Boolean).join('')
}

async function saveHost() {
  const form = hostForm.value
  if (!form.display_name.trim()) {
    Message.error(t('settings.hosts.nameRequired'))
    return
  }
  if (!form.ssh_target.trim()) {
    Message.error(t('settings.hosts.targetRequired'))
    return
  }
  const extraEnv = parseEnvLines(form.extra_env)
  if (!extraEnv) {
    Message.error(t('settings.hosts.extraEnvInvalid'))
    return
  }
  const input: HostInput = {
    display_name: form.display_name.trim(),
    ssh_target: form.ssh_target.trim(),
    registry: form.registry.trim() || null,
    data_dir: form.data_dir.trim() || null,
    extra_env: extraEnv,
  }
  const saved = await saveHostAction.run(hostEditingId.value, input)
  if (saved === undefined) return
  hostSheet.value = false
  await loadHosts()
}

/**
 * Tests one host and shows what it found. A machine that cannot be reached is
 * a result, not a failure: the record stays and the row says 不可达.
 */
async function testHost(host: HostWithStatus) {
  testingHostId.value = host.id
  // 连接中 right away: the answer can take seconds, and the row has to say what
  // is going on while it takes them.
  host.status = { state: 'connecting', message: null }
  try {
    const test = await api.testHostConnection(host.id)
    testResult.value = { hostId: host.id, test }
  } catch (e) {
    Message.error(String(e))
  } finally {
    // The row's status comes from the backend's own record, not from this
    // optimistic 连接中: refresh either way, so a failure shows 不可达 too.
    testingHostId.value = ''
    await loadHosts()
  }
}

function statusLabel(status: HostStatus | null): string {
  if (!status) return t('settings.hosts.statusUnknown')
  return t(`settings.hosts.status.${status.state}`)
}

/** Colour carries state: reachable / unreachable / not asked yet. */
function statusTone(status: HostStatus | null): string {
  if (status?.state === 'reachable') return 'dl-tag--run'
  if (status?.state === 'unreachable') return 'dl-tag--danger'
  return 'dl-tag--muted'
}

function systemLine(test: ConnectionTest): string {
  const system = test.system
  return system ? `${system.os} ${system.release}（${system.arch}）` : '—'
}

/** What the connection test found for one tool; a missing command is said out loud. */
function toolLine(tool: RemoteTool | null): string {
  if (!tool) return t('settings.hosts.toolMissing')
  return tool.version ? `${tool.version} · ${tool.path}` : tool.path
}

function hostName(hostId: string): string {
  return hosts.value.find((h) => h.id === hostId)?.display_name ?? hostId
}

/** The result when the machine answered — the facts below only exist then. */
const passedTest = computed(() => (testResult.value?.test.ok ? testResult.value.test : null))

/** One row per fact the connection test reports: label + mono value. */
const testFacts = computed(() => {
  const test = passedTest.value
  if (!test) return []
  const dataDir = test.data_dir ?? '—'
  return [
    { label: t('settings.hosts.system'), value: systemLine(test) },
    { label: t('settings.hosts.home'), value: test.home ?? '—' },
    {
      label: t('settings.hosts.dataDirResult'),
      value: test.data_dir_exists ? dataDir : `${dataDir} · ${t('settings.hosts.dataDirMissing')}`,
    },
    { label: 'Node', value: toolLine(test.node) },
    { label: 'pnpm', value: toolLine(test.pnpm) },
  ]
})

// --- Host delete ----------------------------------------------------------------

const hostPendingDelete = ref<HostWithStatus | null>(null)
const removeHostAction = useAction((id: string) => api.deleteHost(id), {
  success: () => t('settings.hosts.deleted'),
})

/**
 * The instances of the host being deleted that are still running **on that
 * machine**. Deleting a host deletes the launcher's records and nothing else
 * (ADR-0006 §5, ADR-0007): these processes keep running where they are, with
 * nothing left in the launcher that names them — so the confirmation names them
 * one by one instead of saying 「实例」.
 *
 * 「主机不可达」 counts as running here: the launcher could not ask, and telling
 * the user an instance is gone when it may well be up is the mistake this whole
 * state exists to avoid.
 */
const hostRunsOnDelete = computed(() => {
  const host = hostPendingDelete.value
  if (!host) return []
  return store.instances
    .filter((inst) => (inst.host_id ?? LOCAL_HOST_ID) === host.id)
    .filter((inst) => {
      const state = store.statusOf(inst.id).state
      return state === 'running' || state === 'starting' || state === 'unreachable'
    })
    .map((inst) => inst.name)
})

/** Those names as one phrase for the confirmation. The message names all of
 *  them, so they are joined here rather than passed as an interpolation list
 *  (which would only ever render the first name). */
const hostRunNames = computed(() =>
  hostRunsOnDelete.value.join(locale.value === 'zh-CN' ? '、' : ', '),
)

/** What the user can run on the server to look at what the launcher left
 *  behind. Read-only on purpose: nothing here removes anything there.
 *
 *  The shape is the backend's `remote::ssh_command` + `directory_recipe`, put
 *  together in `directoryCommand` so the two agree — a host whose data directory
 *  has not been read yet falls back to the default spelling `~/.local/share/…`,
 *  and that tilde has to keep expanding on the far side. */
function inspectCommand(host: HostWithStatus | null): string {
  if (!host) return ''
  const dir = host.data_dir || '~/.local/share/dsh-launcher'
  return directoryCommand(host.ssh_target, dir)
}

/**
 * Deleting a host deletes the launcher's records and nothing else. A refusal
 * (「本机是内置的，不能删除」) leaves the sheet open, so the reason stays on screen
 * next to the button the user pressed.
 */
async function confirmDeleteHost() {
  const host = hostPendingDelete.value
  if (!host) return
  await removeHostAction.run(host.id)
  await loadHosts()
  if (hosts.value.some((h) => h.id === host.id)) return
  hostPendingDelete.value = null
  if (testResult.value?.hostId === host.id) testResult.value = null
}

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
  await loadHosts()
  await loadEnv()
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

        <!-- Hosts: the machines the launcher manages. 本机 is built in — no
             status, no test, no delete: there is nothing to dial. A host that
             cannot be reached keeps its record and says 不可达. -->
        <section :ref="(el) => setSectionRef('hosts', el)" class="dl-section">
          <div class="dl-section-label">
            {{ t('settings.hosts.title') }}
            <span class="dl-section-count">{{ hosts.length }}</span>
          </div>
          <div class="dl-surface">
            <div v-for="h in hosts" :key="h.id" class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">
                  {{ h.display_name }}
                  <span v-if="h.builtin" class="dl-tag dl-tag--muted">{{ t('settings.hosts.builtin') }}</span>
                  <span v-else class="dl-tag" :class="statusTone(h.status)">
                    {{ statusLabel(h.status) }}
                  </span>
                </span>
                <span v-if="h.ssh_target" class="dl-row-desc dl-mono">{{ h.ssh_target }}</span>
                <span v-if="h.status?.state === 'unreachable' && h.status.message" class="dl-row-desc">
                  {{ h.status.message }}
                </span>
              </div>
              <div v-if="!h.builtin" class="row-actions">
                <button
                  class="dl-btn dl-btn--quiet"
                  :disabled="testingHostId === h.id"
                  @click="testHost(h)"
                >
                  {{
                    h.status && h.status.state !== 'connecting'
                      ? t('settings.hosts.retest')
                      : t('settings.hosts.test')
                  }}
                </button>
                <button class="dl-btn dl-btn--quiet" @click="startEditHost(h)">
                  {{ t('settings.hosts.edit') }}
                </button>
                <button class="dl-btn dl-btn--ghost" @click="hostPendingDelete = h">
                  {{ t('settings.hosts.remove') }}
                </button>
              </div>
            </div>

            <!-- The last connection test, kept on screen so the user can read a
                 machine's conclusions without re-running it. -->
            <div v-if="testResult" class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">
                  {{ t('settings.hosts.testTitle', { name: hostName(testResult.hostId) }) }}
                  <span
                    class="dl-tag"
                    :class="testResult.test.ok ? 'dl-tag--run' : 'dl-tag--danger'"
                  >
                    {{
                      testResult.test.ok
                        ? t('settings.hosts.status.reachable')
                        : t('settings.hosts.status.unreachable')
                    }}
                  </span>
                </span>
                <span v-if="testResult.test.message" class="dl-row-desc">{{ testResult.test.message }}</span>
              </div>
            </div>
            <div v-for="fact in testFacts" :key="fact.label" class="dl-row">
              <span class="dl-row-label">{{ fact.label }}</span>
              <span class="row-path dl-mono" :title="fact.value">{{ fact.value }}</span>
            </div>

            <div class="dl-row">
              <button class="dl-btn dl-btn--quiet" @click="startAddHost">
                ＋ {{ t('settings.hosts.add') }}
              </button>
            </div>
          </div>
          <p class="section-note">{{ t('settings.hosts.note') }}</p>
          <p v-if="hosts.some((h) => !h.builtin)" class="section-note">
            {{ t('settings.hosts.cannotReach') }}
          </p>
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
            <div v-if="hosts.length > 1" class="dl-row">
              <span class="dl-row-label">{{ t('settings.environment.host') }}</span>
              <a-select
                :model-value="envHostId"
                style="width: 190px"
                size="small"
                @change="(v) => (envHostId = String(v))"
              >
                <a-option v-for="h in hosts" :key="h.id" :value="h.id">{{ h.display_name }}</a-option>
              </a-select>
            </div>
            <!-- A machine that did not answer has no paths to report: the row
                 says why, instead of claiming the tools are missing. -->
            <div v-if="envError" class="dl-row">
              <span class="dl-error-text">{{ envError }}</span>
            </div>
            <div v-if="!envError" class="dl-row">
              <span class="dl-row-label">Node.js</span>
              <span class="row-path dl-mono" :title="envRuntime?.node?.path ?? ''">
                {{ envRuntime?.node?.path ?? t('settings.environment.notFound') }}
              </span>
              <span class="dl-chip">{{ envRuntime?.node?.version ?? t('settings.environment.missing') }}</span>
            </div>
            <div v-if="!envError" class="dl-row">
              <span class="dl-row-label">pnpm</span>
              <span class="row-path dl-mono" :title="envRuntime?.pnpm?.path ?? ''">
                {{ envRuntime?.pnpm?.path ?? t('settings.environment.notFound') }}
              </span>
              <span class="dl-chip">{{ envRuntime?.pnpm?.version ?? t('settings.environment.missing') }}</span>
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

    <!-- Add / edit a remote host. Nothing here is dialed: a record is a
         bookmark, and 「测试连接」 is what talks to the machine. -->
    <a-modal
      :visible="hostSheet"
      :title="hostEditingId ? t('settings.hosts.editTitle') : t('settings.hosts.addTitle')"
      :ok-text="t('settings.hosts.save')"
      :cancel-text="t('common.cancel')"
      :ok-loading="saveHostAction.busy['*']"
      unmount-on-close
      @ok="saveHost"
      @cancel="hostSheet = false"
    >
      <div class="dl-field">
        <span class="dl-field-label">{{ t('settings.hosts.name') }}</span>
        <a-input v-model="hostForm.display_name" :placeholder="t('settings.hosts.namePlaceholder')" />
      </div>
      <div class="dl-field">
        <span class="dl-field-label">{{ t('settings.hosts.target') }}</span>
        <a-select
          v-if="sshAliases.length"
          :model-value="hostForm.ssh_target"
          :placeholder="t('settings.hosts.pickAlias')"
          allow-clear
          allow-search
          @change="(v) => (hostForm.ssh_target = String(v ?? ''))"
        >
          <a-option v-for="a in sshAliases" :key="a.alias" :value="a.alias">
            {{ a.alias }}
            <span v-if="aliasTarget(a)" class="alias-target">{{ aliasTarget(a) }}</span>
          </a-option>
        </a-select>
        <a-input
          v-model="hostForm.ssh_target"
          class="dl-mono"
          :placeholder="t('settings.hosts.targetPlaceholder')"
        />
        <span class="dl-field-hint">{{ t('settings.hosts.targetHint') }}</span>
        <span v-if="!sshAliases.length" class="dl-field-hint">
          {{ t('settings.hosts.pickAliasEmpty') }}
        </span>
      </div>
      <div class="dl-field">
        <span class="dl-field-label">{{ t('settings.hosts.registry') }}</span>
        <a-input v-model="hostForm.registry" :placeholder="t('settings.hosts.registryPlaceholder')" />
        <span class="dl-field-hint">{{ t('settings.hosts.registryHint') }}</span>
      </div>
      <div class="dl-field">
        <span class="dl-field-label">{{ t('settings.hosts.extraEnv') }}</span>
        <a-textarea
          v-model="hostForm.extra_env"
          :placeholder="t('settings.hosts.extraEnvPlaceholder')"
          :auto-size="{ minRows: 2, maxRows: 5 }"
        />
        <span class="dl-field-hint">{{ t('settings.hosts.extraEnvHint') }}</span>
      </div>
      <div class="dl-field">
        <span class="dl-field-label">{{ t('settings.hosts.dataDir') }}</span>
        <a-input
          v-model="hostForm.data_dir"
          class="dl-mono"
          :placeholder="t('settings.hosts.dataDirPlaceholder')"
        />
        <span class="dl-field-hint">{{ t('settings.hosts.dataDirHint') }}</span>
      </div>
    </a-modal>

    <!-- Deleting a host deletes the launcher's records and nothing else: the
         warning says what stays on the machine, and hands over a command the
         user can run there themselves (ADR-0006 §5). -->
    <a-modal
      :visible="!!hostPendingDelete"
      :title="t('settings.hosts.deleteTitle', { name: hostPendingDelete?.display_name ?? '' })"
      :ok-text="t('settings.hosts.remove')"
      :cancel-text="t('common.cancel')"
      :ok-loading="removeHostAction.busy['*']"
      @ok="confirmDeleteHost"
      @cancel="hostPendingDelete = null"
    >
      <p class="delete-warning">{{ t('settings.hosts.deleteWarning') }}</p>
      <p v-if="hostRunsOnDelete.length" class="delete-warning">
        {{ t('settings.hosts.deleteRunning', { names: hostRunNames }) }}
      </p>
      <p class="delete-warning">{{ t('settings.hosts.deleteRecords') }}</p>
      <p class="delete-warning">{{ t('settings.hosts.deleteLeft') }}</p>
      <div class="cleanup-command">
        <code class="dl-mono">{{ inspectCommand(hostPendingDelete) }}</code>
        <button
          class="dl-btn dl-btn--quiet"
          @click="copy(inspectCommand(hostPendingDelete), 'settings.hosts.deleteInspectCopied')"
        >
          {{ t('common.copy') }}
        </button>
      </div>
    </a-modal>
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

// The alias picker shows what an alias points at, as a muted suffix: the alias
// is what the user recognises, the target is what the launcher will dial.
.alias-target {
  margin-left: 8px;
  color: var(--dl-ink-4);
  font-family: var(--dl-font-mono);
  font-size: var(--dl-text-xs);
}

.delete-warning {
  margin: 0 0 10px;
  font-size: var(--dl-text-sm);
  line-height: 1.6;
  color: var(--dl-ink-2);
}

.cleanup-command {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface-2);

  code {
    flex: 1;
    min-width: 0;
    font-size: var(--dl-text-xs);
    color: var(--dl-ink-2);
    overflow-wrap: anywhere;
  }
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

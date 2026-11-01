<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { LOCAL_HOST_ID } from '@/api/types'
import type { RuntimeStatus, SkippedCandidate, ToolSource } from '@/api/types'
import { useCopy } from '@/composables/useCopy'
import { useLauncherStore } from '@/stores/launcher'
import DlIcon from '@/components/DlIcon.vue'

// The toolchain check. It states the same version floors the backend resolver
// enforces and offers the install command the backend picked for this machine,
// so the page never invents a number or a package manager.
//
// The verdict is per source: whether the tool comes from the user's login-shell
// PATH (the one a terminal would find), from a directory the launcher searched
// on its own, or from a path the user picked. Anything but `system` means the
// terminal does not see it, so those tools also get the picker.

type ToolId = 'node' | 'pnpm'

const TOOL_IDS: ToolId[] = ['node', 'pnpm']
const TOOL_LABELS: Record<ToolId, string> = { node: 'Node.js', pnpm: 'pnpm' }

const router = useRouter()
const { copy } = useCopy()
const store = useLauncherStore()

const checking = ref(false)
const picking = ref<ToolId | null>(null)

// The page answers for one host at a time — the launcher's 当前主机, the same
// state the Library's picker and the board's filter read, so this page reports
// the machine those pages are showing (ADR-0006 §3). 本机 is the default, and it
// is the only host whose answer the app itself needs — the redirect here at
// startup is about the launcher's own toolchain (App.vue), so only that answer
// is mirrored into the store.
const hostId = computed({
  get: () => store.currentHostId,
  set: (id: string) => void store.selectHost(id),
})
// Only 本机's answer is known when the page opens; a machine that has not been
// probed starts empty, so its panel never wears the local conclusion while the
// read is in flight.
const runtime = ref<RuntimeStatus | null>(
  store.currentHostId === LOCAL_HOST_ID ? store.runtime : null,
)
const manualInput = ref<Record<ToolId, string>>({ node: '', pnpm: '' })
/** Why this host has no conclusions at all (an unreachable machine, a bad target). */
const loadError = ref('')

const selectedHost = computed(() => store.currentHost)
const isRemote = computed(() => hostId.value !== LOCAL_HOST_ID)

function statusOf(id: ToolId) {
  return (id === 'node' ? runtime.value?.node : runtime.value?.pnpm) ?? null
}

interface ToolRow {
  id: ToolId
  name: string
  ok: boolean
  version: string | null
  path: string | null
  source: ToolSource | null
}

const tools = computed<ToolRow[]>(() =>
  TOOL_IDS.map((id) => {
    const status = statusOf(id)
    return {
      id,
      name: TOOL_LABELS[id],
      ok: status?.installed ?? false,
      version: status?.version ?? null,
      path: status?.path ?? null,
      source: status?.source ?? null,
    }
  }),
)

const nodeOk = computed(() => statusOf('node')?.installed ?? false)
const pnpmOk = computed(() => statusOf('pnpm')?.installed ?? false)
// Node is what DSH needs to boot; pnpm is needed to install a version and to
// manage plugins, and the launcher no longer installs one itself.
const allOk = computed(() => nodeOk.value && pnpmOk.value)
const belowRecommended = computed(
  () => runtime.value?.node_below_recommended ?? false,
)

const requirements = computed(() => runtime.value?.requirements)
const nodeCommand = computed(
  () => requirements.value?.node_install_command ?? '',
)
const pnpmCommand = computed(
  () => requirements.value?.pnpm_install_command ?? '',
)
// The commands the backend hands out are already the ones for the selected
// machine (on a remote host each is prefixed with `ssh <host>`), so the copy
// only has to say where they are run.
const nodeCommandDesc = computed(() =>
  isRemote.value
    ? '在本机终端执行以下命令；它会经 ssh 连到那台机器，在它的交互式登录 shell 里安装 Node.js：'
    : '在终端执行以下命令安装 Node.js：',
)
const pnpmCommandDesc = computed(() => {
  const major = requirements.value?.min_pnpm_major
  return isRemote.value
    ? `那台机器上需要 pnpm ${major} 或更高。在本机终端执行以下命令；它会经 ssh 连到那台机器，在它的交互式登录 shell 里安装：`
    : `启动器需要 pnpm ${major} 或更高版本。在终端执行以下命令安装：`
})

// The candidates the probe passed over on its way to a conclusion: on a remote
// host an old Node is passed over rather than warned about, and the user is
// owed the reason (ADR-0005 追加).
const skipped = computed<SkippedCandidate[]>(
  () => runtime.value?.skipped_candidates ?? [],
)

function skippedReason(candidate: SkippedCandidate): string {
  if (candidate.reason === 'below-minimum') {
    return `${candidate.path} 的 ${candidate.version ?? ''} 低于 ${candidate.major ?? ''}，已跳过`
  }
  if (candidate.reason === 'unparsable') {
    return `${candidate.path} 的版本号「${candidate.version ?? ''}」读不出来，已跳过`
  }
  return `${candidate.path} 用不了，已跳过`
}

const overridePath = (id: ToolId) => statusOf(id)?.manual_path ?? null
// A pick that no longer resolves: the backend falls back to its own detection
// and still reports the chosen path, so the page can say so and let the user
// replace or clear it.
const overrideBroken = (id: ToolId) => {
  const status = statusOf(id)
  return !!status?.manual_path && status.source !== 'manual'
}

// The tools the picker is worth showing for: a terminal would not find them
// (missing, or found by the launcher on its own), or the user already picked a
// path and must be able to see, change and clear it — including a pick that has
// gone stale while the tool itself is back in the system PATH.
const manualTools = computed(() =>
  tools.value.filter(
    (tool) => !tool.ok || tool.source !== 'system' || !!overridePath(tool.id),
  ),
)

function sourceLabel(tool: ToolRow): string {
  if (!tool.ok) return `系统环境变量里没有，常用安装目录里也没有`
  if (tool.source === 'system') return `来自系统环境变量`
  if (tool.source === 'fallback')
    return `系统环境变量里没有，启动器在常用安装目录里找到的`
  if (tool.source === 'manual') return `你手动指定的路径`
  return ''
}

const copied = ref('')

async function copyCommand(text: string, key: string) {
  if (!(await copy(text))) return
  copied.value = key
  window.setTimeout(() => {
    if (copied.value === key) copied.value = ''
  }, 2000)
}

// One answer at a time: a load started before the user switched hosts must not
// land on the page as if it were about the host now selected.
let loadToken = 0

async function load(force = false) {
  const wanted = hostId.value
  const token = ++loadToken
  try {
    const answer = await api.getRuntimeStatus(force, wanted)
    if (token !== loadToken) return
    loadError.value = ''
    runtime.value = answer
    if (answer.host_id === LOCAL_HOST_ID) store.runtime = answer
  } catch (e) {
    if (token !== loadToken) return
    // A machine that cannot be reached has no conclusions to show. Reporting
    // "Node 未安装" for it would send the user to install a toolchain on a
    // machine that already has one.
    loadError.value = String(e)
    runtime.value = null
  }
}

// The one current host can move from another page too (the Library's picker,
// the board's filter), and its conclusion is that machine's own. Clearing first
// keeps the machine just left — its tools, or the reason it failed — from being
// read as the new host's answer while the probe runs.
watch(hostId, () => {
  runtime.value = null
  loadError.value = ''
  void load()
})

/** Re-reads the environment: the user may have just fixed their PATH. */
async function recheck() {
  checking.value = true
  try {
    await load(true)
    if (nodeOk.value && pnpmOk.value) Message.success(`环境已就绪`)
  } finally {
    checking.value = false
  }
}

// A toolchain installed in a terminal becomes visible as soon as the user comes
// back to the window, so the page does not make them hunt for a button.
async function onFocus() {
  if (checking.value) return
  await load()
}

/** Points one tool at the file the user picked; `null` goes back to automatic detection. */
async function applyOverride(id: ToolId, path: string | null) {
  try {
    const answer = await api.setToolchainOverride(id, path, hostId.value)
    runtime.value = answer
    if (answer.host_id === LOCAL_HOST_ID) store.runtime = answer
    manualInput.value[id] = ''
    Message.success(
      path ? `已指定 ${TOOL_LABELS[id]}` : `已取消指定 ${TOOL_LABELS[id]}`,
    )
  } catch (e) {
    Message.error(String(e))
  }
}

/**
 * A remote host has no file picker on this machine, so the path is typed and the
 * backend checks it by running `--version` over there.
 */
function submitManual(id: ToolId) {
  const raw = manualInput.value[id].trim()
  if (!raw) return
  return applyOverride(id, raw)
}

async function pickFile(id: ToolId) {
  if (!api.isTauri) {
    Message.info(`浏览器预览不支持系统文件选择，请在桌面版里指定`)
    return
  }
  picking.value = id
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    // No filters: node and pnpm carry no file extension, so a filter would hide them.
    const picked = await open({ multiple: false })
    if (typeof picked === 'string') await applyOverride(id, picked)
  } catch (e) {
    Message.error(String(e))
  } finally {
    picking.value = null
  }
}

function clearOverride(id: ToolId) {
  return applyOverride(id, null)
}

onMounted(async () => {
  window.addEventListener('focus', onFocus)
  await load()
})
onUnmounted(() => window.removeEventListener('focus', onFocus))
</script>

<template>
  <div class="dl-page setup-page">
    <header class="dl-head">
      <div class="dl-head-text">
        <h1>{{ `环境检测` }}</h1>
        <p class="dl-head-sub">
          {{ `启动器以系统 PATH 为准检测 Node.js 与 pnpm。` }}
        </p>
      </div>
      <div class="dl-head-actions">
        <a-select
          v-if="store.hosts.length > 1"
          v-model="hostId"
          style="width: 190px"
          size="small"
        >
          <a-option v-for="h in store.hosts" :key="h.id" :value="h.id">{{
            h.display_name
          }}</a-option>
        </a-select>
        <button
          class="dl-btn dl-btn--quiet"
          :disabled="checking"
          @click="recheck"
        >
          <DlIcon name="refresh" :size="13" />
          {{ `重新检测` }}
        </button>
        <button
          v-if="allOk"
          class="dl-btn dl-btn--primary"
          @click="router.push({ name: 'instances' })"
        >
          {{ `进入启动器` }}
        </button>
      </div>
    </header>

    <!-- Status first: the two tools, resolved paths and what to do if a leg is
         missing. One block per missing tool, so a machine lacking both is told
         both things at once. -->
    <section class="dl-section">
      <div class="dl-section-label">
        {{ `环境` }}
        <span class="dl-section-count">
          <span
            class="dl-tag"
            :class="allOk ? 'dl-tag--run' : 'dl-tag--danger'"
          >
            {{ allOk ? `环境已就绪` : `环境未就绪` }}
          </span>
        </span>
      </div>

      <p v-if="loadError" class="setup-note setup-note--quiet">
        {{ loadError }}
      </p>

      <div v-if="!loadError" class="dl-surface">
        <div v-for="tool in tools" :key="tool.name" class="dl-row">
          <span class="tool-mark" :class="tool.ok ? 'is-on' : 'is-off'">
            <DlIcon :name="tool.ok ? 'check' : 'alert'" :size="13" />
          </span>
          <div class="tool-info">
            <span class="tool-name">{{ tool.name }}</span>
            <span v-if="tool.ok && tool.path" class="tool-path dl-mono">{{
              tool.path
            }}</span>
            <span class="tool-source">{{ sourceLabel(tool) }}</span>
          </div>
          <span class="dl-chip">{{ tool.ok ? tool.version : `未安装` }}</span>
        </div>
      </div>

      <p v-if="isRemote" class="setup-note setup-note--quiet">
        {{
          `这里的结论来自「${selectedHost?.display_name ?? ''}」自己的交互式登录 shell（就是你在那台机器的终端里敲命令时命中的那一套），不是本机的。`
        }}
      </p>

      <p v-if="nodeOk && belowRecommended" class="setup-note">
        <DlIcon name="alert" :size="13" />
        {{ `建议升级到 ${requirements?.recommended_node_major} 或更高` }}
      </p>

      <!-- On a remote host an old candidate is passed over instead of warned
           about, so the page has to say which ones it walked past and why. -->
      <div v-if="skipped.length" class="dl-surface skipped">
        <div class="skipped-head">{{ `被跳过的候选` }}</div>
        <div
          v-for="candidate in skipped"
          :key="candidate.tool + candidate.path"
          class="skipped-row"
        >
          <span class="dl-chip">{{ candidate.tool }}</span>
          <span class="skipped-why">{{ skippedReason(candidate) }}</span>
        </div>
      </div>
    </section>

    <!-- The two ways out of a missing or non-system toolchain: point the
         launcher at the file, or fix PATH in a terminal and re-check. Shown for
         every tool the terminal would not find, plus any tool the user already
         picked, so their choice stays visible and revisable. -->
    <section v-if="!loadError && manualTools.length" class="dl-section">
      <div class="dl-section-label">{{ `手动指定可执行文件` }}</div>

      <div v-for="tool in manualTools" :key="tool.id" class="dl-surface pick">
        <div class="pick-head">
          <span class="pick-title">{{ `指定 ${tool.name} 的可执行文件` }}</span>
          <span class="pick-desc">{{
            `可以直接选择 ${tool.name} 的可执行文件；也可以去终端把它加进 PATH（例如写进 ~/.zshrc），回来点「重新检测」。`
          }}</span>
          <span v-if="overridePath(tool.id)" class="pick-path dl-mono">
            {{ overridePath(tool.id) }}
          </span>
          <span v-if="overrideBroken(tool.id)" class="pick-broken">
            <DlIcon name="alert" :size="13" />
            {{ `指定的路径已不可用，已改回自动检测` }}
          </span>
        </div>
        <div class="pick-actions">
          <template v-if="isRemote">
            <input
              v-model="manualInput[tool.id]"
              class="dl-input dl-mono"
              :placeholder="`/usr/local/bin/node`"
              @keyup.enter="submitManual(tool.id)"
            />
            <button
              class="dl-btn dl-btn--quiet"
              :disabled="!manualInput[tool.id].trim()"
              @click="submitManual(tool.id)"
            >
              {{ `使用这个路径` }}
            </button>
          </template>
          <button
            v-else
            class="dl-btn dl-btn--quiet"
            :disabled="picking === tool.id"
            @click="pickFile(tool.id)"
          >
            <DlIcon name="folder" :size="13" />
            {{ `选择可执行文件` }}
          </button>
          <button
            v-if="overridePath(tool.id)"
            class="dl-btn dl-btn--quiet"
            :disabled="picking === tool.id"
            @click="clearOverride(tool.id)"
          >
            {{ `取消指定` }}
          </button>
        </div>
      </div>
    </section>

    <section v-if="!loadError && !allOk" class="dl-section">
      <div class="dl-section-label">{{ `怎么装` }}</div>

      <div v-if="!nodeOk" class="dl-surface guide">
        <div class="guide-head">
          <span class="guide-title">{{ `安装 Node.js` }}</span>
          <span class="guide-desc">{{ nodeCommandDesc }}</span>
        </div>
        <div class="cmd-row">
          <code class="cmd-text">{{ nodeCommand }}</code>
          <button
            class="dl-btn dl-btn--quiet"
            @click="copyCommand(nodeCommand, 'node')"
          >
            <DlIcon name="copy" :size="13" />
            {{ copied === 'node' ? `已复制` : `复制` }}
          </button>
        </div>
      </div>

      <div v-if="!pnpmOk" class="dl-surface guide">
        <div class="guide-head">
          <span class="guide-title">{{ `安装 pnpm` }}</span>
          <span class="guide-desc">{{ pnpmCommandDesc }}</span>
        </div>
        <div class="cmd-row">
          <code class="cmd-text">{{ pnpmCommand }}</code>
          <button
            class="dl-btn dl-btn--quiet"
            @click="copyCommand(pnpmCommand, 'pnpm')"
          >
            <DlIcon name="copy" :size="13" />
            {{ copied === 'pnpm' ? `已复制` : `复制` }}
          </button>
        </div>
      </div>

      <p class="setup-note">
        {{
          `上面的命令都执行完后，切回本窗口即会自动重新检测；也可以点页头的「重新检测」强制重读一次系统环境变量。`
        }}
      </p>
    </section>
  </div>
</template>

<style lang="scss" scoped>
.setup-page {
  max-width: 720px;
}

.tool-mark {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  flex-shrink: 0;
  border-radius: var(--dl-r-ctl);

  &.is-on {
    background: var(--dl-run-soft);
    color: var(--dl-run);
  }

  &.is-off {
    background: var(--dl-danger-soft);
    color: var(--dl-danger);
  }
}

.tool-info {
  display: flex;
  flex-direction: column;
  gap: 1px;
  flex: 1;
  min-width: 0;
}

.tool-name {
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.tool-path {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tool-source {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.setup-note {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 10px 2px 0;
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-warn);

  &--quiet {
    color: var(--dl-ink-3);
  }
}

.skipped {
  margin-top: 12px;
  padding: 14px 16px;
}

.skipped-head {
  margin-bottom: 8px;
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.skipped-row {
  display: flex;
  align-items: center;
  gap: 8px;

  & + .skipped-row {
    margin-top: 6px;
  }
}

.skipped-why {
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
  word-break: break-all;
}

.guide {
  padding: 16px;

  & + .guide {
    margin-top: 12px;
  }
}

.pick {
  padding: 16px;

  & + .pick {
    margin-top: 12px;
  }
}

.pick-head {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-bottom: 12px;
}

.pick-title {
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.pick-desc {
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
}

.pick-path {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pick-broken {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-warn);
}

.pick-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.guide-head {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-bottom: 12px;
}

.guide-title {
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.guide-desc {
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
}

.cmd-row {
  display: flex;
  align-items: stretch;
  gap: 8px;
}

.cmd-text {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  padding: 10px 12px;
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface-3);
  color: var(--dl-ink);
  font-family: var(--dl-font-mono);
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-all;
  user-select: text;
}
</style>

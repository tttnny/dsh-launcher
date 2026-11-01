<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { LOCAL_HOST_ID } from '@/api/types'
import type { RuntimeStatus, SkippedCandidate, ToolSource } from '@/api/types'
import { useCopy } from '@/composables/useCopy'
import { useLauncherStore } from '@/stores/launcher'
import DlIcon from '@/components/DlIcon.vue'

// 一台机器的运行环境：Node.js 与 pnpm 从哪来、装在哪个路径、缺了怎么装，
// 以及把启动器指向用户自己的可执行文件。结论按主机读取，切换主机时旧的
// 答案先清空，避免把上一台机器的结论当成这一台的。

type ToolId = 'node' | 'pnpm'

const TOOL_IDS: ToolId[] = ['node', 'pnpm']
const TOOL_LABELS: Record<ToolId, string> = { node: 'Node.js', pnpm: 'pnpm' }

const props = defineProps<{ hostId: string }>()

const store = useLauncherStore()
const { copy } = useCopy()

const runtime = ref<RuntimeStatus | null>(null)
const loadError = ref('')
const checking = ref(false)
const picking = ref<ToolId | null>(null)
const manualInput = ref<Record<ToolId, string>>({ node: '', pnpm: '' })
const copied = ref('')

const hostName = computed(
  () => store.hostById(props.hostId)?.display_name ?? '这台机器',
)
const isRemote = computed(() => props.hostId !== LOCAL_HOST_ID)

const detecting = computed(() => !runtime.value && !loadError.value)

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
  manualPath: string | null
  broken: boolean
}

const toolRows = computed<ToolRow[]>(() =>
  TOOL_IDS.map((id) => {
    const status = statusOf(id)
    return {
      id,
      name: TOOL_LABELS[id],
      ok: status?.installed ?? false,
      version: status?.version ?? null,
      path: status?.path ?? null,
      source: status?.source ?? null,
      manualPath: status?.manual_path ?? null,
      broken: !!status?.manual_path && status?.source !== 'manual',
    }
  }),
)

const nodeOk = computed(() => statusOf('node')?.installed ?? false)
const pnpmOk = computed(() => statusOf('pnpm')?.installed ?? false)
const allOk = computed(() => nodeOk.value && pnpmOk.value)
const belowRecommended = computed(
  () => runtime.value?.node_below_recommended ?? false,
)

const envState = computed(() => {
  if (loadError.value) return { label: '无法检测', tone: 'tag--warn' }
  if (!runtime.value) return { label: '检测中…', tone: 'tag--muted' }
  return allOk.value
    ? { label: '环境已就绪', tone: 'tag--run' }
    : { label: '环境未就绪', tone: 'tag--danger' }
})

const requirements = computed(() => runtime.value?.requirements)
const skipped = computed<SkippedCandidate[]>(
  () => runtime.value?.skipped_candidates ?? [],
)
const missingTools = computed(() => toolRows.value.filter((tool) => !tool.ok))
const manualRows = computed(() =>
  toolRows.value.filter(
    (tool) => !tool.ok || tool.source !== 'system' || !!tool.manualPath,
  ),
)

function commandOf(id: ToolId): string {
  return id === 'node'
    ? (requirements.value?.node_install_command ?? '')
    : (requirements.value?.pnpm_install_command ?? '')
}

function commandDesc(id: ToolId): string {
  if (isRemote.value) {
    return id === 'node'
      ? '在本机终端执行以下命令；它会经 ssh 连到那台机器安装 Node.js。'
      : '在本机终端执行以下命令；它会经 ssh 连到那台机器安装 pnpm。'
  }
  return id === 'node'
    ? '在终端执行以下命令安装 Node.js。'
    : '在终端执行以下命令安装 pnpm。'
}

function sourceLabel(tool: ToolRow): string {
  if (!tool.ok) return '系统环境变量里没有，常用安装目录里也没有'
  if (tool.source === 'system') return '来自系统环境变量'
  if (tool.source === 'fallback') return '启动器在常用安装目录里找到的'
  if (tool.source === 'manual') return '你手动指定的路径'
  return ''
}

function skippedReason(candidate: SkippedCandidate): string {
  if (candidate.reason === 'below-minimum') {
    return (
      candidate.path +
      ' 的 ' +
      (candidate.version ?? '') +
      ' 低于 ' +
      (candidate.major ?? '') +
      '，已跳过'
    )
  }
  if (candidate.reason === 'unparsable') {
    return (
      candidate.path +
      ' 的版本号「' +
      (candidate.version ?? '') +
      '」读不出来，已跳过'
    )
  }
  return candidate.path + ' 用不了，已跳过'
}

let loadToken = 0

async function load(force = false) {
  const wanted = props.hostId
  const token = ++loadToken
  try {
    const answer = await api.getRuntimeStatus(force, wanted)
    if (token !== loadToken) return
    loadError.value = ''
    runtime.value = answer
    if (answer.host_id === LOCAL_HOST_ID) store.runtime = answer
  } catch (e) {
    if (token !== loadToken) return
    // 连不上的机器没有结论：显示「无法检测」，绝不把工具说成「未安装」。
    loadError.value = String(e)
    runtime.value = null
  }
}

watch(
  () => props.hostId,
  () => {
    runtime.value = null
    loadError.value = ''
    void load()
  },
)

async function recheck() {
  checking.value = true
  try {
    await load(true)
    if (nodeOk.value && pnpmOk.value) Message.success('环境已就绪')
  } finally {
    checking.value = false
  }
}

async function copyCommand(id: ToolId) {
  const text = commandOf(id)
  if (!text) return
  if (!(await copy(text, '安装命令已复制'))) return
  copied.value = id
  window.setTimeout(() => {
    if (copied.value === id) copied.value = ''
  }, 2000)
}

async function applyOverride(id: ToolId, path: string | null) {
  try {
    const answer = await api.setToolchainOverride(id, path, props.hostId)
    runtime.value = answer
    if (answer.host_id === LOCAL_HOST_ID) store.runtime = answer
    manualInput.value[id] = ''
    Message.success(
      path ? '已指定 ' + TOOL_LABELS[id] : '已取消指定 ' + TOOL_LABELS[id],
    )
  } catch (e) {
    Message.error(String(e))
  }
}

function submitManual(id: ToolId) {
  const raw = manualInput.value[id].trim()
  if (!raw) return
  return applyOverride(id, raw)
}

async function pickFile(id: ToolId) {
  if (!api.isTauri) {
    Message.info('浏览器预览不支持系统文件选择，请在桌面版里指定')
    return
  }
  picking.value = id
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
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

async function onFocus() {
  if (checking.value) return
  await load()
}

onMounted(() => {
  window.addEventListener('focus', onFocus)
  void load()
})
onUnmounted(() => {
  window.removeEventListener('focus', onFocus)
})
</script>

<template>
  <section class="section">
    <div class="section-head">
      <span class="section-title">运行环境</span>
      <span class="tag" :class="envState.tone">{{ envState.label }}</span>
      <span class="section-actions">
        <button
          class="btn btn--quiet btn--sm"
          :disabled="checking"
          @click="recheck"
        >
          <DlIcon name="refresh" :size="12" />
          重新检测
        </button>
      </span>
    </div>

    <div v-if="loadError" class="notice notice--warn env-error">
      <div class="notice-body">
        <div class="notice-title">无法检测</div>
        <div>{{ loadError }}</div>
      </div>
      <button
        class="btn btn--quiet btn--sm"
        :disabled="checking"
        @click="recheck"
      >
        重试
      </button>
    </div>

    <template v-else>
      <div class="surface">
        <div v-for="tool in toolRows" :key="tool.id" class="list-row">
          <span
            class="tool-state"
            :class="detecting ? 'is-pending' : tool.ok ? 'is-on' : 'is-off'"
          >
            <DlIcon
              :name="detecting ? 'refresh' : tool.ok ? 'check' : 'alert'"
              :size="13"
            />
          </span>
          <div class="row-main">
            <div class="row-title">
              <span>{{ tool.name }}</span>
              <span
                v-if="tool.ok && tool.version"
                class="tag tag--muted mono"
                >{{ tool.version }}</span
              >
              <span v-else-if="!detecting" class="tag tag--danger">未安装</span>
            </div>
            <div v-if="tool.broken" class="error-text">
              指定的路径已不可用，已改回自动检测
            </div>
          </div>
          <div class="row-actions">
            <button
              v-if="tool.manualPath"
              class="btn btn--ghost btn--sm"
              :disabled="picking === tool.id"
              @click="clearOverride(tool.id)"
            >
              取消指定
            </button>
          </div>
        </div>
      </div>

      <p v-if="nodeOk && belowRecommended" class="env-note">
        <DlIcon name="alert" :size="13" />
        建议升级到 Node.js {{ requirements?.recommended_node_major }} 或更高
      </p>

      <div v-if="!detecting && missingTools.length" class="env-block">
        <div
          v-for="tool in missingTools"
          :key="tool.id"
          class="surface surface--pad guide"
        >
          <div class="guide-head">
            <span class="field-label">安装 {{ tool.name }}</span>
            <span class="field-hint">{{ commandDesc(tool.id) }}</span>
          </div>
          <div class="cmd-row">
            <code class="cmd mono">{{ commandOf(tool.id) }}</code>
            <button
              class="btn btn--quiet btn--sm"
              @click="copyCommand(tool.id)"
            >
              <DlIcon name="copy" :size="12" />
              {{ copied === tool.id ? '已复制' : '复制' }}
            </button>
          </div>
        </div>
      </div>

      <div v-if="!detecting && manualRows.length" class="env-block">
        <div
          v-for="tool in manualRows"
          :key="tool.id"
          class="surface surface--pad manual-card"
        >
          <div class="manual-head">
            <span class="field-label">指定 {{ tool.name }} 的可执行文件</span>
            <span v-if="tool.manualPath" class="field-hint">
              当前指定：<span class="mono">{{ tool.manualPath }}</span>
            </span>
          </div>
          <div class="manual-actions">
            <template v-if="isRemote">
              <input
                v-model="manualInput[tool.id]"
                class="input input--mono"
                placeholder="/usr/local/bin/node"
                @keyup.enter="submitManual(tool.id)"
              />
              <button
                class="btn btn--quiet btn--sm"
                :disabled="!manualInput[tool.id].trim()"
                @click="submitManual(tool.id)"
              >
                使用这个路径
              </button>
            </template>
            <button
              v-else
              class="btn btn--quiet btn--sm"
              :disabled="picking === tool.id"
              @click="pickFile(tool.id)"
            >
              <DlIcon name="folder" :size="12" />
              选择可执行文件
            </button>
            <button
              v-if="tool.manualPath"
              class="btn btn--ghost btn--sm"
              :disabled="picking === tool.id"
              @click="clearOverride(tool.id)"
            >
              取消指定
            </button>
          </div>
        </div>
      </div>

      <div
        v-if="!detecting && skipped.length"
        class="surface surface--pad skipped"
      >
        <div class="field-label skipped-title">被跳过的候选</div>
        <div
          v-for="candidate in skipped"
          :key="candidate.tool + candidate.path"
          class="skipped-row"
        >
          <span class="tag tag--muted">{{ candidate.tool }}</span>
          <span class="skipped-why">{{ skippedReason(candidate) }}</span>
        </div>
      </div>
    </template>
  </section>
</template>

<style lang="scss" scoped>
.env-error {
  align-items: center;
}

.tool-state {
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

  &.is-pending {
    background: var(--dl-surface-2);
    color: var(--dl-ink-3);
  }
}

.env-path {
  max-width: 46ch;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.env-note {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 10px 2px 0;
  color: var(--dl-warn);
  font-size: var(--dl-text-sm);
  line-height: 1.5;

  &--quiet {
    color: var(--dl-ink-3);
  }
}

.env-block {
  margin-top: 12px;
}

.guide + .guide {
  margin-top: 10px;
}

.guide-head {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-bottom: 10px;
}

.cmd-row {
  display: flex;
  align-items: stretch;
  gap: 8px;
}

.cmd {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  padding: 9px 11px;
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface-3);
  color: var(--dl-ink);
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-all;
  user-select: text;
}

.manual-card + .manual-card {
  margin-top: 10px;
}

.manual-head {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-bottom: 10px;
}

.manual-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.skipped {
  margin-top: 12px;
}

.skipped-title {
  margin-bottom: 8px;
}

.skipped-row {
  display: flex;
  align-items: baseline;
  gap: 8px;

  & + .skipped-row {
    margin-top: 6px;
  }
}

.skipped-why {
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}
</style>

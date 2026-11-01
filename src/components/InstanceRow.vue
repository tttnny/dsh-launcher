<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore, type HealthLogItem } from '@/stores/launcher'
import type { DshInstance } from '@/api/types'
import { useAction } from '@/composables/useAction'
import { useCopy } from '@/composables/useCopy'
import { usePluginLink } from '@/composables/usePluginLink'
import { useProfiles } from '@/composables/useProfiles'
import DlIcon from '@/components/DlIcon.vue'
import InstanceAvatar from '@/components/InstanceAvatar.vue'

// One instance on the board.
//
// The row answers two questions — what is it, and is it live — and carries the
// controls that act on those answers: the profile it launches with, its primary
// action, and, while a process is live, that process's own restart and stop.
// Facts and the remaining actions live in the card the row expands into;
// expanding the row is for reading, not for reaching a control.

const props = defineProps<{
  inst: DshInstance
  /** Set when something handed the board a reason to point at this row. */
  focus?: boolean
}>()
const emit = defineEmits<{ changed: []; duplicate: [id: string] }>()

const router = useRouter()
const { copy } = useCopy()
const store = useLauncherStore()
const {
  profilesByHome,
  selectionByInstance,
  loadingByHome,
  ensureSelection,
  invalidateHome,
} = useProfiles()
const { canManagePlugins, openPlugins } = usePluginLink()

// --- Derived facts ------------------------------------------------------------

const status = computed(() => store.statusOf(props.inst.id))

const STATUS_LABELS: Record<string, string> = {
  stopped: '已停止',
  starting: '启动中',
  running: '运行中',
  exited: '已退出',
  unreachable: '主机不可达',
}
const expanded = ref(false)
const rowEl = ref<HTMLElement | null>(null)
const highlighted = ref(false)
function toggleExpanded() {
  expanded.value = !expanded.value
}

/**
 * Comes back expanded and briefly ringed, so "返回实例" lands on the row the
 * user left instead of a board they have to search again.
 */
function reveal() {
  expanded.value = true
  void nextTick(() => {
    rowEl.value?.scrollIntoView({ block: 'center', behavior: 'smooth' })
    highlighted.value = true
    window.setTimeout(() => (highlighted.value = false), 1600)
  })
}

watch(
  () => props.focus,
  (v) => {
    if (v) reveal()
  },
  { immediate: true },
)

/**
 * The whole row is the hit target, but the controls inside it are not: a click
 * or a keypress that belongs to a button, the profile select, or a link is that
 * control's, not the disclosure's. Keyboard matters here because Enter on a
 * focused button fires the button *and* bubbles this far — without the guard,
 * stopping an instance from the row would also expand it.
 */
function onRowControl(target: EventTarget | null) {
  return !!(target as HTMLElement | null)?.closest(
    'button, a, input, textarea, select, .arco-select',
  )
}

function onRowClick(event: MouseEvent) {
  if (onRowControl(event.target)) return
  toggleExpanded()
}

function onRowKeydown(event: KeyboardEvent) {
  if (onRowControl(event.target)) return
  event.preventDefault()
  toggleExpanded()
}
const running = computed(() => status.value.state === 'running')
const starting = computed(() => status.value.state === 'starting')
/**
 * 「主机不可达」: the machine that runs this instance did not answer, so its state
 * is unknown. It is deliberately not 已停止 — that reading is what makes a user
 * start a second DSH on the server (ADR-0007). The row's answer is therefore a
 * retry, never a 启动.
 */
const unreachable = computed(() => status.value.state === 'unreachable')
/**
 * A readable name for what the row binds. The store can be missing a resource
 * when that machine's list could not be read, so the row says what it does not
 * know in words; the internal id stays on the element's title.
 */
const versionName = computed(
  () => store.versionById(props.inst.version_id)?.version ?? `未知版本`,
)
/**
 * Installing a version names its instance after that version, so the common
 * case has the name and the version saying the same thing twice. The name is
 * the row's title; the chip is only worth showing when it adds something.
 */
const showVersionChip = computed(() => versionName.value !== props.inst.name)
const homeName = computed(
  () => store.homeById(props.inst.home_id)?.name ?? `未知 DSH_HOME`,
)
const sharedHome = computed(() => store.isHomeShared(props.inst.home_id))
const version = computed(() => store.versionById(props.inst.version_id))
const home = computed(() => store.homeById(props.inst.home_id))
const host = computed(() => store.hostById(props.inst.host_id))
/**
 * Two instances called 「web」 on two machines must not read alike, so a row that
 * belongs to another machine says which one — but only where more than one
 * machine is on screen (the board's 全部 view). Under a single-host filter every
 * row would otherwise repeat the same name the filter already states.
 */
const showHost = computed(
  () => store.allHosts && !!host.value && !host.value.builtin,
)
/**
 * On another machine, so its DSH_HOME is a directory there. The three machine
 * actions still apply — a terminal connects to it, its log is copied home — but
 * 「打开目录」 cannot open what this Mac does not have.
 */
const remote = computed(() => !!host.value && !host.value.builtin)
/**
 * Why the state is what it is, when the state itself cannot say: the last word
 * about reaching that machine (連不上 / 认证失败 / 超时), or the retry's own
 * explanation.
 */
const statusHint = computed(() => {
  if (!unreachable.value) return undefined
  return host.value?.status?.message || `再向那台机器问一次这个实例的状态。`
})
/**
 * The one-line reason a row shows while its machine is unreachable: the common
 * failures as everyday words, anything else as the first sentence of what the
 * machine said. The full message stays on the status chip's title.
 */
const failureSummary = computed(() => {
  const message = host.value?.status?.message || ''
  if (!message) return `这台机器没有回应`
  if (/connection refused|econnrefused/i.test(message)) return `连接被拒绝`
  if (/timed out|timeout|超时/i.test(message)) return `连接超时`
  if (/permission denied|publickey|authentication|认证/i.test(message))
    return `认证失败`
  if (/could not resolve|name or service not known|resolve/i.test(message))
    return `找不到这台机器`
  if (/no route to host|network is unreachable|unreachable/i.test(message))
    return `网络不通`
  const first = message.split(/[。\n]/)[0].trim()
  return first.length > 24 ? `${first.slice(0, 24)}…` : first
})
const envEntries = computed(() =>
  Object.entries(props.inst.env_overrides ?? {}),
)
const portLabel = computed(() =>
  props.inst.port ? String(props.inst.port) : `随机端口`,
)
/**
 * Where a remote instance really is, as that machine printed it. It is stated
 * next to the address that works here, because the two are different addresses
 * whenever this Mac already had the port — and 「打开」 is always the one that
 * works here.
 */
const remoteUrl = computed(() => status.value.remote_url ?? null)
/** The in-app self-check entries recorded for this instance. */
const health = computed(() => {
  const mine = store.healthLogs.filter((l) => l.instanceId === props.inst.id)
  return {
    errors: mine.filter((l) => l.level === 'error').length,
    warns: mine.filter((l) => l.level === 'warn').length,
  }
})
const profiles = computed(() => profilesByHome[props.inst.home_id] ?? [])
const profilesLoading = computed(() => !!loadingByHome[props.inst.home_id])

const selectedProfile = computed<string | undefined>({
  get: () => selectionByInstance[props.inst.id],
  set: (v) => {
    selectionByInstance[props.inst.id] = v
  },
})

// --- Profile selection --------------------------------------------------------

// An unreachable machine cannot answer for its Profile list, so the row does
// not ask for one: there is no empty select, and no "no Profile" warning fires.
watch(
  () => [props.inst.home_id, unreachable.value] as const,
  () => {
    if (unreachable.value) return
    void ensureSelection(props.inst).then((p) => {
      if (p === undefined)
        Message.warning(`该实例的 DSH_HOME 下没有可用 Profile`)
    })
  },
  { immediate: true },
)

async function refreshProfiles() {
  invalidateHome(props.inst.home_id)
  await ensureSelection(props.inst)
}

// --- Lifecycle ----------------------------------------------------------------

const startAction = useAction(
  (args: { id: string; profile: string }) =>
    api.startInstance(args.id, args.profile),
  { key: (args) => args.id, success: () => `实例启动命令已发送` },
)
const stopAction = useAction((id: string) => api.stopInstance(id), {
  key: (id) => id,
  success: () => `实例已停止`,
})
const restartAction = useAction((id: string) => api.restartInstance(id), {
  key: (id) => id,
  success: () => `实例启动命令已发送`,
})
const openAction = useAction((id: string) => api.openInstanceWindow(id), {
  key: (id) => id,
})
/**
 * Ask that machine once more. The answer arrives as a status event (and the
 * host's own message is re-read so the row can say *why* it was unreachable),
 * so the click needs no toast of its own.
 */
const retryAction = useAction(
  async (id: string) => {
    await api.refreshInstanceStatus(id)
    store.hosts = await api.listHosts()
    return true
  },
  { key: (id) => id },
)

const busy = computed(
  () =>
    !!startAction.busy[props.inst.id] ||
    !!stopAction.busy[props.inst.id] ||
    !!restartAction.busy[props.inst.id],
)

function touchLastUsed() {
  if (store.settings.last_instance_id === props.inst.id) return
  void api.updateSettings({ last_instance_id: props.inst.id }).then((s) => {
    store.settings = s
  })
}

async function reportHealth(profile: string) {
  try {
    const report = await api.checkInstanceHealth(props.inst.id, profile)
    if (!report?.findings?.length) return
    const logs: HealthLogItem[] = report.findings.map((f, idx) => ({
      id: `${Date.now()}-${props.inst.id}-${idx}-${Math.random().toString(36).slice(2, 6)}`,
      timestamp: Date.now(),
      instanceId: props.inst.id,
      instanceName: props.inst.name,
      profile,
      level: f.level,
      code: f.code,
      message: f.message,
    }))
    store.addHealthLogs(logs)
  } catch {
    // Advisory only
  }
}

async function onStart() {
  const profile = selectedProfile.value
  if (!profile || busy.value) return
  const ok = await startAction.run({ id: props.inst.id, profile })
  if (ok === undefined) return
  touchLastUsed()
  void reportHealth(profile)
}

async function onOpen() {
  const ok = await openAction.run(props.inst.id)
  if (ok === undefined) return
  touchLastUsed()
}

async function onStop() {
  await stopAction.run(props.inst.id)
}

async function onRetry() {
  await retryAction.run(props.inst.id)
}

async function onRestart() {
  const ok = await restartAction.run(props.inst.id)
  if (ok === undefined) return
  const profile = status.value.profile ?? selectedProfile.value
  if (profile) void reportHealth(profile)
}

// --- Utilities ----------------------------------------------------------------

const dirAction = useAction(() => api.openInstanceDirectory(props.inst.id), {
  success: (path) => `已在访达中打开目录：${path}`,
})
const logAction = useAction(() => api.openInstanceLog(props.inst.id), {
  success: (path) => `已在访达中定位日志：${path}`,
})
const terminalAction = useAction(
  () => api.openInstanceTerminal(props.inst.id),
  {
    success: (label) => `已打开终端：${label}`,
  },
)

// An instance on another machine has a DSH_HOME that is a directory *there*:
// this Mac can neither open it nor hold it, so the machine line hands over what
// a user can actually use — the path, and one command that reaches it.
const remoteCopyAction = useAction(async (what: 'path' | 'command') => {
  const location = await api.instanceRemoteLocation(props.inst.id)
  await copy(
    what === 'path' ? location.path : location.command,
    what === 'path' ? '远端路径已复制' : '命令已复制，粘到终端里就能连上去',
  )
})

function openDetail() {
  void router
    .push({ name: 'instance-detail', params: { id: props.inst.id } })
    .catch(() => undefined)
}

// Removing the binding, from the board: the same command the settings page's
// danger zone runs, so the version and the DSH_HOME stay exactly as they are.
const deleteAction = useAction(
  async (id: string) => {
    await api.deleteInstance(id, props.inst.host_id)
    return true
  },
  { key: (id) => id },
)

function confirmDelete() {
  Modal.confirm({
    title: `删除实例「${props.inst.name}」？`,
    content: `只删除启动器里的这条实例记录：DSH 版本和 DSH_HOME「${homeName.value}」都会保留。实例专属图标会一并删除。`,
    okText: `删除`,
    cancelText: `取消`,
    okButtonProps: { status: 'danger' },
    modalClass: 'dl-sheet',
    onOk: async () => {
      const ok = await deleteAction.run(props.inst.id)
      if (ok !== true) return
      await store.refreshInstances()
      Message.success(`已删除实例「${props.inst.name}」`)
      emit('changed')
    },
  })
}

onMounted(() => {
  if (status.value.state === 'running') {
    void ensureSelection(props.inst).then((p) => {
      if (p) void reportHealth(p)
    })
  }
})
</script>

<template>
  <div
    ref="rowEl"
    class="inst-row"
    :class="{
      'is-live': running,
      'is-starting': starting,
      'is-expanded': expanded,
      'is-focus': highlighted,
    }"
  >
    <div
      class="inst-line"
      tabindex="0"
      @click="onRowClick"
      @keydown.enter.space="onRowKeydown"
    >
      <InstanceAvatar :id="inst.id" :icon="inst.icon" :size="36" />

      <div class="inst-main">
        <div class="inst-title-line">
          <button
            class="inst-name"
            :title="inst.name"
            :aria-expanded="expanded"
            @click.stop="toggleExpanded"
          >
            {{ inst.name }}
          </button>
          <span v-if="sharedHome" class="dl-tag dl-tag--warn">{{
            `共享 DSH_HOME`
          }}</span>
        </div>
        <div class="inst-meta">
          <template v-if="showHost">
            <span :title="host?.ssh_target || host?.display_name">{{
              host?.display_name
            }}</span>
            <span class="inst-meta-sep" />
          </template>
          <template v-if="showVersionChip">
            <span class="dl-mono" :title="version?.dir || inst.version_id">{{
              versionName
            }}</span>
            <span class="inst-meta-sep" />
          </template>
          <span class="inst-meta-home" :title="home?.path || inst.home_id">
            {{ homeName }}
          </span>
        </div>
      </div>

      <!-- The row's controls: what the instance runs on, its primary action,
         and — only while the process is live — that process's own controls as
         icon buttons on the same line, so every state keeps one row height. -->
      <div class="inst-tail">
        <div class="inst-tail-line">
          <div class="inst-profile">
            <template v-if="!running && !starting && !unreachable">
              <span class="inst-profile-label">{{ `Profile` }}</span>
              <a-select
                v-model="selectedProfile"
                :placeholder="`选择 Profile`"
                :loading="profilesLoading"
                size="small"
                :allow-clear="false"
                style="width: 132px"
                @change="touchLastUsed"
              >
                <a-option v-for="p in profiles" :key="p" :value="p">{{
                  p
                }}</a-option>
              </a-select>
            </template>
            <!-- An unreachable machine has no Profile list to offer: the row
               says why it could not be read instead of opening an empty select. -->
            <span
              v-else-if="unreachable"
              class="inst-failure"
              :title="statusHint"
            >
              <DlIcon name="alert" :size="12" />
              {{ failureSummary }}
            </span>
            <template v-else>
              <span class="inst-profile-label">{{ `Profile` }}</span>
              <span class="dl-chip" :title="status.profile ?? selectedProfile">
                {{ status.profile ?? selectedProfile ?? `选择 Profile` }}
              </span>
            </template>
          </div>

          <span
            class="dl-status inst-status"
            :class="`is-${status.state}`"
            :title="statusHint"
          >
            {{ STATUS_LABELS[status.state] }}
          </span>

          <div class="inst-actions">
            <button
              v-if="running"
              class="dl-btn dl-btn--live inst-primary"
              :disabled="!!openAction.busy[inst.id]"
              @click="onOpen"
            >
              <DlIcon name="external" :size="13" />
              {{ `打开` }}
            </button>
            <button
              v-else-if="starting"
              class="dl-btn dl-btn--quiet inst-primary"
              disabled
            >
              <span class="dl-spinner" />
              {{ `启动中` }}
            </button>
            <!-- Unknown is not stopped: the only useful move is to ask again, and
               a 启动 here is exactly the click that creates a second DSH. -->
            <button
              v-else-if="unreachable"
              class="dl-btn dl-btn--primary-soft inst-primary"
              :title="statusHint"
              :disabled="retryAction.busy[inst.id]"
              @click="onRetry"
            >
              <DlIcon name="refresh" :size="13" />
              {{ `重试` }}
            </button>
            <button
              v-else
              class="dl-btn dl-btn--primary-soft inst-primary"
              :disabled="
                !selectedProfile || busy || !store.versionById(inst.version_id)
              "
              @click="onStart"
            >
              <DlIcon name="play" :size="13" />
              {{ `启动` }}
            </button>
            <!-- The process controls, on the primary action's own line. 停止 is
               offered while starting too, so a launch that hangs is not a dead end. -->
            <template v-if="running || starting">
              <button
                v-if="running"
                class="dl-icon-btn inst-proc-btn"
                :title="`重启`"
                :disabled="busy"
                @click="onRestart"
              >
                <DlIcon name="restart" :size="14" />
              </button>
              <button
                class="dl-icon-btn inst-proc-btn inst-stop"
                :title="`停止`"
                :disabled="!!stopAction.busy[inst.id]"
                @click="onStop"
              >
                <DlIcon name="stop" :size="14" />
              </button>
            </template>
          </div>
        </div>
      </div>

      <!-- The disclosure mark, at the far edge where it reads as "this row opens"
         rather than as another button next to the name. -->
      <DlIcon
        name="chevron-down"
        :size="15"
        class="inst-chev"
        :class="{ 'is-open': expanded }"
      />
    </div>

    <!-- Expanded: the facts that otherwise cost a trip to the settings page,
         and the actions that have no business being in a hover menu. -->
    <div v-if="expanded" class="inst-facts">
      <div class="facts-grid">
        <div class="fact">
          <span class="fact-label">{{ `DSH 版本` }}</span>
          <span
            class="fact-value dl-mono"
            :title="version?.dir || inst.version_id"
            >{{ versionName }}</span
          >
        </div>
        <div class="fact">
          <span class="fact-label">{{ `DSH_HOME` }}</span>
          <span class="fact-value" :title="home?.path || inst.home_id">
            {{ homeName }}
            <span class="fact-sub dl-mono">{{ home?.path }}</span>
          </span>
        </div>
        <div class="fact">
          <span class="fact-label">{{ `默认 Profile` }}</span>
          <span class="fact-value">{{ inst.default_profile || `未设置` }}</span>
        </div>
        <div class="fact">
          <span class="fact-label">{{ `端口号` }}</span>
          <span class="fact-value dl-mono">{{ portLabel }}</span>
        </div>
        <!-- The live address belongs with the facts, not on the row: opening and
           copying it live here, so the row itself keeps one height. A remote
           instance's address is the forwarded one; 「远端位置」 states where it
           really runs. -->
        <div v-if="remoteUrl" class="fact">
          <span class="fact-label">{{ `远端位置` }}</span>
          <span class="fact-value fact-chips">
            <span class="fact-chip dl-mono" :title="remoteUrl">{{
              remoteUrl
            }}</span>
            <button
              class="dl-icon-btn"
              :title="`复制`"
              @click="copy(remoteUrl!)"
            >
              <DlIcon name="copy" :size="12" />
            </button>
          </span>
        </div>
        <div v-if="status.url" class="fact">
          <span class="fact-label">{{ `本机地址` }}</span>
          <span class="fact-value fact-chips">
            <button
              class="fact-link dl-mono"
              :title="`在浏览器里打开`"
              @click="onOpen"
            >
              {{ status.url }}
            </button>
            <button
              class="dl-icon-btn"
              :title="`复制`"
              @click="copy(status.url!)"
            >
              <DlIcon name="copy" :size="12" />
            </button>
          </span>
        </div>
        <div class="fact">
          <span class="fact-label">
            {{ `环境变量覆写` }}
            <span v-if="envEntries.length" class="fact-count">{{
              envEntries.length
            }}</span>
          </span>
          <span class="fact-value fact-chips">
            <template v-if="envEntries.length">
              <span
                v-for="[key, value] in envEntries"
                :key="key"
                class="fact-chip dl-mono"
                :title="`${key}=${value}`"
              >
                {{ key }}
              </span>
            </template>
            <span v-else class="fact-sub">{{ `无` }}</span>
          </span>
        </div>
        <div class="fact">
          <span class="fact-label">{{ `自检` }}</span>
          <span class="fact-value">
            <span v-if="health.errors > 0" class="fact-bad">
              {{ `${health.errors} 个错误` }}
            </span>
            <span v-else-if="health.warns > 0" class="fact-warn">
              {{ `${health.warns} 个警告` }}
            </span>
            <!-- The dependency tree of a remote instance is on that machine, and
               the launcher does not read it: 「没有发现问题」 would be a claim it
               never checked. -->
            <span v-else-if="remote" class="fact-sub">{{
              `远端实例还没有自检过`
            }}</span>
            <span v-else class="fact-sub">{{ `没有异常` }}</span>
          </span>
        </div>
      </div>

      <!-- The action bar is grouped by what an action acts on: the machine,
           then this binding. A small title and a divider state the two groups;
           the delete sits apart at the far end of the binding group. -->
      <div class="facts-actions">
        <div class="facts-group">
          <span class="facts-group-title">{{ `这台机器` }}</span>
          <div class="facts-line">
            <button class="dl-btn dl-btn--quiet" @click="terminalAction.run()">
              <DlIcon name="terminal" :size="13" />
              {{ `打开终端` }}
            </button>
            <!-- This Mac's file manager cannot open a directory on another
                 machine, so that machine's DSH_HOME is handed over as what it
                 is: a path to paste, and one command that reaches it. -->
            <template v-if="remote">
              <button
                class="dl-btn dl-btn--quiet"
                @click="remoteCopyAction.run('path')"
              >
                <DlIcon name="copy" :size="13" />
                {{ `复制路径` }}
              </button>
              <button
                class="dl-btn dl-btn--quiet"
                @click="remoteCopyAction.run('command')"
              >
                <DlIcon name="copy" :size="13" />
                {{ `复制命令` }}
              </button>
            </template>
            <button
              v-else
              class="dl-btn dl-btn--quiet"
              @click="dirAction.run()"
            >
              <DlIcon name="folder" :size="13" />
              {{ `打开目录` }}
            </button>
            <button class="dl-btn dl-btn--quiet" @click="logAction.run()">
              <DlIcon name="log" :size="13" />
              {{ `查看日志` }}
            </button>
          </div>
        </div>

        <div class="facts-divider" />

        <!-- Everything here acts on the binding itself, never on this process
             or this machine. The settings page is the same three facts,
             editable; a copy is a new binding of the same version + HOME. -->
        <div class="facts-group">
          <span class="facts-group-title">{{ `这个实例绑定` }}</span>
          <div class="facts-line">
            <button class="dl-btn dl-btn--quiet" @click="openDetail">
              <DlIcon name="settings" :size="13" />
              {{ `实例设置` }}
            </button>
            <button
              class="dl-btn dl-btn--quiet"
              @click="emit('duplicate', inst.id)"
            >
              <DlIcon name="copy" :size="13" />
              {{ `复制实例` }}
            </button>
            <button
              v-if="canManagePlugins(inst.id)"
              class="dl-btn dl-btn--quiet"
              @click="openPlugins(inst.id, 'instances')"
            >
              <DlIcon name="plugin" :size="13" />
              {{ `管理插件` }}
            </button>
            <button
              v-if="!running && !starting"
              class="dl-btn dl-btn--quiet"
              :disabled="profilesLoading"
              @click="refreshProfiles"
            >
              <DlIcon name="refresh" :size="13" />
              {{ `重新读取 Profile 列表` }}
            </button>
            <!-- Removing the binding, set apart at the far end: only the record
                 goes; the version and the DSH_HOME are shared resources. -->
            <button
              class="dl-btn dl-btn--danger facts-delete"
              :disabled="!!deleteAction.busy[inst.id]"
              @click="confirmDelete"
            >
              <DlIcon name="trash" :size="13" />
              {{ `删除` }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.inst-row {
  position: relative;

  // Rows are divided by an inset separator, the way macOS draws list rows.
  & + .inst-row::before {
    content: '';
    position: absolute;
    top: 0;
    left: 18px;
    right: 0;
    height: 1px;
    background: var(--dl-line);
  }

  // Arriving back from the plugin manager: the row says "here", once.
  &.is-focus .inst-line {
    animation: inst-focus 1.6s var(--dl-ease);
  }
}

@keyframes inst-focus {
  from {
    background-color: var(--dl-signal-soft);
  }
}
// The row proper; the facts panel below it is a sibling so a row can grow
// downward without turning into a nested flex mess.
.inst-line {
  display: flex;
  align-items: center;
  gap: 14px;
  min-height: 62px;
  padding: 13px 16px 13px 18px;
  cursor: pointer;
  transition: background-color var(--dl-fast) var(--dl-ease);

  &:hover {
    background-color: var(--dl-surface-2);
  }

  &:focus-visible {
    outline: 2px solid var(--dl-signal-line);
    outline-offset: -3px;
  }
}

.inst-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.inst-title-line {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.inst-name {
  padding: 0;
  border: none;
  background: none;
  color: var(--dl-ink);
  font-family: inherit;
  font-size: var(--dl-text-lg);
  font-weight: 600;
  letter-spacing: -0.018em;
  cursor: pointer;
  min-width: 0;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;

  &:hover {
    color: var(--dl-signal);
  }
}

// The disclosure mark: at the row's outer edge, quiet until the row is under
// the cursor or open, so it reads as part of the row rather than a control.
.inst-chev {
  flex-shrink: 0;
  margin-left: 2px;
  margin-right: -2px;
  color: var(--dl-ink-4);
  transition:
    transform var(--dl-slow) var(--dl-ease),
    color var(--dl-fast) var(--dl-ease);

  &.is-open {
    transform: rotate(180deg);
  }
}

.inst-line:hover .inst-chev,
.inst-row.is-expanded .inst-chev {
  color: var(--dl-ink-2);
}

// The expanded fact sheet: a quiet second surface inside the row.
.inst-facts {
  padding: 12px 16px 16px 82px;
  background: var(--dl-surface-2);
  border-top: 1px solid var(--dl-line);
  animation: facts-in var(--dl-fast) var(--dl-ease);
}

.facts-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
  gap: 10px 24px;
}

.facts-actions {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 10px;
  margin-top: 14px;
}

.facts-group {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

// A quiet group title, so the machine actions and the binding actions read as
// two sets rather than one long toolbar.
.facts-group-title {
  font-size: var(--dl-text-xs);
  font-weight: 600;
  color: var(--dl-ink-3);
}

.facts-divider {
  height: 1px;
  background: var(--dl-line);
}

// One line per group, each wrapping inside itself: a group never breaks in the
// middle of another, so no button is ever stranded on a line of its own.
.facts-line {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
  width: 100%;
}

// The destructive action is pushed to the far end of its own group.
.facts-delete {
  margin-left: auto;
}

@keyframes facts-in {
  from {
    opacity: 0;
    transform: translateY(-3px);
  }
}

.fact {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.fact-label {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-4);
}

.fact-count {
  padding: 0 4px;
  border-radius: 4px;
  background: var(--dl-surface-3);
  color: var(--dl-ink-3);
}

.fact-value {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.fact-sub {
  display: block;
  color: var(--dl-ink-4);
  overflow: hidden;
  text-overflow: ellipsis;
}

// One line under the facts, for what the two addresses above mean: the row
// states them, and this says which one a click uses.
.facts-note {
  margin: 10px 0 0;
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-4);
}

.fact-chips {
  display: flex;
  gap: 6px;
  overflow: hidden;
}

.fact-chip {
  padding: 0 6px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface);
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-2);
}

.fact-bad {
  color: var(--dl-danger);
}

.fact-warn {
  color: var(--dl-warn);
}

.inst-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.inst-meta-sep {
  width: 3px;
  height: 3px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--dl-ink-4);
}

.inst-meta-home {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

// An address that opens on click, rendered as a chip in the facts grid.
.fact-link {
  padding: 0 6px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface);
  color: var(--dl-run);
  font-size: var(--dl-text-xs);
  cursor: pointer;
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;

  &:hover:not(:disabled) {
    text-decoration: underline;
  }
}

// The row's control block: one line, right-aligned, in every state.
.inst-tail {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  flex-shrink: 0;
}

.inst-tail-line {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
}

// Stopping is the one control on the row that takes something away; it says so
// on hover instead of wearing red all the time.
.inst-stop:hover:not(:disabled) {
  background: var(--dl-danger-soft);
  color: var(--dl-danger);
}

.inst-profile {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

// The dropdown's visible name, so 「web」 is not mistaken for a state or version.
.inst-profile-label {
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-3);
}

// Why an unreachable machine's row cannot offer a Profile: a short reason in
// place of the select, with the full message on hover.
.inst-failure {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 220px;
  color: var(--dl-warn);
  font-size: var(--dl-text-sm);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.inst-status {
  width: 62px;
  flex-shrink: 0;
}

.inst-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

// The live process's own controls sit beside the primary action as icon buttons.
.inst-proc-btn {
  width: 26px;
  height: 26px;
}

.inst-primary {
  min-width: 84px;
}

.dl-spinner {
  width: 11px;
  height: 11px;
  border: 2px solid currentColor;
  border-right-color: transparent;
  border-radius: 50%;
  animation: dl-spin 0.7s linear infinite;
}

@keyframes dl-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>

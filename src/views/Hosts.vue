<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { LOCAL_HOST_ID } from '@/api/types'
import type {
  ConnectionTest,
  HostInput,
  HostStatus,
  HostWithStatus,
  RemoteTool,
  SshAlias,
} from '@/api/types'
import { useAction } from '@/composables/useAction'
import { useCopy } from '@/composables/useCopy'
import { useLauncherStore } from '@/stores/launcher'
import DlIcon from '@/components/DlIcon.vue'

// 主机 is the machines themselves — the axis every instance, version, DSH_HOME
// and Profile is filed under. This page is where a machine is added, edited,
// tested and deleted, and where its footprint (how many instances, versions and
// HOMEs live there) is read at a glance. The sidebar rail stays a switch; this
// page is the record and its controls, so managing a machine is not buried in a
// settings scroll and the toolchain check is one click from the machine it is
// about.

const router = useRouter()
const store = useLauncherStore()
const { copy } = useCopy()

// --- Host list ----------------------------------------------------------------

const hosts = ref<HostWithStatus[]>([])
/** Why the host list could not be read; empty when the last read succeeded. */
const hostsError = ref('')
const sshAliases = ref<SshAlias[]>([])

async function loadHosts() {
  try {
    const list = await api.listHosts()
    hosts.value = list
    hostsError.value = ''
    // The same list the rail, the Library picker and the board read: an add,
    // rename or delete here has to reach them, and so does the fallback when
    // the current host is the one that went away.
    store.setHosts(list)
    // Every machine's footprint, so a card can count its versions and HOMEs
    // without being opened first. A failed read stays absent and the card says
    // so; it never serves the previous answer.
    for (const host of list) {
      if (!store.versionsByHost[host.id] && !store.versionErrors[host.id]) {
        void store.loadHostResources(host.id)
      }
    }
  } catch (e) {
    hostsError.value = String(e)
  }
}

// --- Per-card footprint -------------------------------------------------------

const instanceCount = (hostId: string) => store.instancesOfHost(hostId).length

/** Versions read from the machine; undefined = not read (or the read failed),
 * so the card says 「—」 instead of inventing a zero. */
function versionsLabel(hostId: string): string {
  if (store.versionErrors[hostId]) return '—'
  const list = store.versionsByHost[hostId]
  return list ? String(list.length) : '—'
}

function homesLabel(hostId: string): string {
  if (store.homeErrors[hostId]) return '—'
  const list = store.homesByHost[hostId]
  return list ? String(list.length) : '—'
}

/** The data directory a machine keeps the launcher's records in. */
function dataDirLabel(host: HostWithStatus): string {
  return host.data_dir || '默认目录'
}

/** 本机 is the only machine whose toolchain the launcher checks on its own at
 *  startup; a remote machine's is read on demand from 环境检测. */
const localEnvOk = computed(
  () => !!store.runtime?.node?.installed && !!store.runtime?.pnpm?.installed,
)

// --- Status -------------------------------------------------------------------

const HOST_STATUS_LABELS: Record<string, string> = {
  connecting: '连接中',
  reachable: '可达',
  unreachable: '不可达',
}

function statusLabel(status: HostStatus | null): string {
  if (!status) return '未测试'
  return HOST_STATUS_LABELS[status.state]
}

function statusTone(status: HostStatus | null): string {
  if (status?.state === 'reachable') return 'dl-tag--run'
  if (status?.state === 'unreachable') return 'dl-tag--warn'
  return 'dl-tag--muted'
}

/** The status dot: steady, one colour, no pulse — 本机 reads as reachable by
 *  definition because it has no dial to make. */
function dotTone(host: HostWithStatus): string {
  if (host.builtin) return 'is-local'
  const st = host.status?.state
  return st === 'reachable'
    ? 'is-ok'
    : st === 'connecting'
      ? 'is-busy'
      : st === 'unreachable'
        ? 'is-warn'
        : 'is-idle'
}

interface HostErrorText {
  reason: string
  advice: string
  raw: string
}

function parseHostError(message: string): HostErrorText {
  const raw = message.trim()
  let reason = '连不上这台主机'
  let advice = '确认这台机器开着、地址没写错，再点「重试」。'
  if (/refused/i.test(raw)) {
    reason = '这台主机拒绝了连接'
  } else if (/timed out|time out|timeout/i.test(raw)) {
    reason = '连接这台主机超时'
    advice = '确认这台机器能连上网络，再点「重试」。'
  } else if (/no route|unreachable|network is down/i.test(raw)) {
    reason = '网络不通，连不到这台主机'
    advice = '确认这台机器在网络里、地址没写错，再点「重试」。'
  } else if (/permission denied|authentication/i.test(raw)) {
    reason = '这台主机拒绝了登录'
    advice = '确认登录账号和密钥可用，再点「重试」。'
  } else if (/could not resolve|name or service not known/i.test(raw)) {
    reason = '找不到这台主机'
    advice = '确认地址拼写正确、网络能查到这台机器，再点「重试」。'
  }
  return { reason, advice, raw }
}

/** The unreachable machines' error copy, keyed by host id. */
const hostErrors = computed<Record<string, HostErrorText>>(() => {
  const map: Record<string, HostErrorText> = {}
  for (const host of hosts.value) {
    if (host.status?.state === 'unreachable' && host.status.message?.trim()) {
      map[host.id] = parseHostError(host.status.message)
    }
  }
  return map
})

// --- Connection test ----------------------------------------------------------

const testingHostId = ref('')
/** The last connection-test result, with the host it belongs to. */
const testResult = ref<{ hostId: string; test: ConnectionTest } | null>(null)

async function testHost(host: HostWithStatus) {
  testingHostId.value = host.id
  // 连接中 right away: the answer can take seconds, and the card has to say
  // what is going on while it takes them.
  host.status = { state: 'connecting', message: null }
  try {
    const test = await api.testHostConnection(host.id)
    testResult.value = { hostId: host.id, test }
  } catch (e) {
    Message.error(String(e))
  } finally {
    testingHostId.value = ''
    await loadHosts()
  }
}

function systemLine(test: ConnectionTest): string {
  const system = test.system
  return system ? `${system.os} ${system.release}（${system.arch}）` : '—'
}

function toolLine(tool: RemoteTool | null): string {
  if (!tool) return `没有这个命令`
  return tool.version ? `${tool.version} · ${tool.path}` : tool.path
}

function dataDirLine(test: ConnectionTest): string {
  if (test.data_dir_problem) return test.data_dir_problem
  const dir = test.data_dir ?? '—'
  return test.data_dir_exists
    ? dir
    : `${dir} · ${'还不存在（启动器第一次写入时会创建）'}`
}

/** The facts the connection test reports, only once the machine answered. */
function testFacts(test: ConnectionTest) {
  return [
    { label: '系统', value: systemLine(test) },
    { label: '家目录', value: test.home ?? '—' },
    { label: '数据目录', value: dataDirLine(test) },
    { label: 'Node', value: toolLine(test.node) },
    { label: 'pnpm', value: toolLine(test.pnpm) },
  ]
}

// --- Add / edit ---------------------------------------------------------------

interface HostForm {
  display_name: string
  ssh_target: string
  registry: string
  data_dir: string
  extra_env: string
}

function emptyHostForm(): HostForm {
  return {
    display_name: '',
    ssh_target: '',
    registry: '',
    data_dir: '',
    extra_env: '',
  }
}

const hostSheet = ref(false)
const hostEditingId = ref('')
const hostForm = ref<HostForm>(emptyHostForm())
/** Editing the built-in 本机: it has no ssh target, so the form drops that field
 *  and the save does not demand one. */
const editingBuiltin = computed(
  () => !!hosts.value.find((h) => h.id === hostEditingId.value)?.builtin,
)
const hostSheetTitle = computed(() =>
  hostEditingId.value
    ? editingBuiltin.value
      ? `编辑本机`
      : `编辑主机`
    : `添加远程主机`,
)

const saveHostAction = useAction(
  (id: string, input: HostInput) =>
    id ? api.updateHost(id, input) : api.createHost(input),
  { success: () => `主机已保存` },
)

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

function aliasTarget(alias: SshAlias): string {
  const host = alias.user
    ? `${alias.user}@${alias.host_name ?? alias.alias}`
    : (alias.host_name ?? '')
  return [host, alias.port ? `:${alias.port}` : ''].filter(Boolean).join('')
}

async function saveHost() {
  const form = hostForm.value
  if (!form.display_name.trim()) {
    Message.error(`请填写主机的显示名`)
    return
  }
  if (!editingBuiltin.value && !form.ssh_target.trim()) {
    Message.error(
      `请填写 ssh 目标：~/.ssh/config 里的别名，或 user@host 这样的字符串`,
    )
    return
  }
  const extraEnv = parseEnvLines(form.extra_env)
  if (!extraEnv) {
    Message.error(`环境变量要写成每行一个 KEY=VALUE`)
    return
  }
  const input: HostInput = {
    display_name: form.display_name.trim(),
    ssh_target: editingBuiltin.value ? '' : form.ssh_target.trim(),
    registry: form.registry.trim() || null,
    data_dir: form.data_dir.trim() || null,
    extra_env: extraEnv,
  }
  const saved = await saveHostAction.run(hostEditingId.value, input)
  if (saved === undefined) return
  hostSheet.value = false
  await loadHosts()
}

// --- Delete -------------------------------------------------------------------

const hostPendingDelete = ref<HostWithStatus | null>(null)
const removeHostAction = useAction((id: string) => api.deleteHost(id), {
  success: () => `主机记录已删除`,
})

/**
 * The instances of the host being deleted that are still running **on that
 * machine**. Deleting a host deletes the launcher's records and nothing else:
 * these processes keep running where they are, so the confirmation names them
 * one by one. 「主机不可达」 counts as running — the launcher could not ask.
 */
const hostRunsOnDelete = computed(() => {
  const host = hostPendingDelete.value
  if (!host) return []
  return store.instances
    .filter((inst) => (inst.host_id ?? LOCAL_HOST_ID) === host.id)
    .filter((inst) => {
      const state = store.statusOf(inst.id).state
      return (
        state === 'running' || state === 'starting' || state === 'unreachable'
      )
    })
    .map((inst) => inst.name)
})

const hostRunNames = computed(() => hostRunsOnDelete.value.join('、'))

const inspectCommand = ref('')
const inspectCommandError = ref('')
let inspectCommandSeq = 0

watch(hostPendingDelete, async (host) => {
  const seq = ++inspectCommandSeq
  inspectCommand.value = ''
  inspectCommandError.value = ''
  if (!host) return
  const dir = host.data_dir || '~/.local/share/dsh-launcher'
  try {
    const command = await api.hostDirectoryCommand(host.id, dir)
    if (seq !== inspectCommandSeq) return
    inspectCommand.value = command
  } catch (e) {
    if (seq !== inspectCommandSeq) return
    inspectCommandError.value = String(e)
  }
})

async function confirmDeleteHost() {
  const host = hostPendingDelete.value
  if (!host) return
  await removeHostAction.run(host.id)
  await loadHosts()
  if (hosts.value.some((h) => h.id === host.id)) return
  hostPendingDelete.value = null
  if (testResult.value?.hostId === host.id) testResult.value = null
}

// --- Jumps into the machine's own surfaces -----------------------------------

/** 环境检测 answers for the current host, so selecting this machine first is
 *  what makes the page it opens be about the same machine this card is. */
function goEnvironment(host: HostWithStatus) {
  void store.selectHost(host.id)
  void router.push({ name: 'setup' })
}

/** 资源库 shows the current host's versions, HOMEs and profiles. */
function goResources(host: HostWithStatus) {
  void store.selectHost(host.id)
  void router.push({ name: 'library' })
}

onMounted(loadHosts)
</script>

<template>
  <div class="dl-page hosts-page">
    <header class="dl-head">
      <div class="dl-head-text">
        <h1>{{ `主机` }}</h1>
        <p class="dl-head-sub">
          {{ `每台机器各自拥有自己的版本、DSH_HOME、Profile 与运行环境。` }}
        </p>
      </div>
      <div class="dl-head-actions">
        <button class="dl-btn dl-btn--primary" @click="startAddHost">
          <DlIcon name="plus" :size="13" />
          {{ `添加主机` }}
        </button>
      </div>
    </header>

    <div v-if="hostsError" class="dl-surface dl-failure">
      <p class="dl-failure-title">{{ `读不到主机列表` }}</p>
      <p class="dl-failure-text">{{ hostsError }}</p>
      <p class="dl-failure-hint">{{ `修好之后再试一次。` }}</p>
      <button class="dl-btn dl-btn--quiet" @click="loadHosts">
        <DlIcon name="refresh" :size="13" />
        {{ `重新读取` }}
      </button>
    </div>

    <div v-else class="host-list">
      <article v-for="h in hosts" :key="h.id" class="host-card dl-surface">
        <header class="host-card-head">
          <span class="host-dot" :class="dotTone(h)" />
          <div class="host-id">
            <div class="host-name-line">
              <span class="host-name">{{ h.display_name }}</span>
              <span v-if="h.builtin" class="dl-tag dl-tag--muted">{{
                `内置`
              }}</span>
              <span v-else class="dl-tag" :class="statusTone(h.status)">{{
                statusLabel(h.status)
              }}</span>
              <span
                v-if="h.builtin"
                class="dl-tag"
                :class="localEnvOk ? 'dl-tag--run' : 'dl-tag--danger'"
              >
                {{ localEnvOk ? `环境就绪` : `环境未就绪` }}
              </span>
            </div>
            <span
              v-if="h.ssh_target"
              class="host-target dl-mono"
              :title="h.ssh_target"
              >{{ h.ssh_target }}</span
            >
            <span v-else class="host-target">{{ `这台 Mac` }}</span>
          </div>
          <div class="host-actions">
            <button
              v-if="!h.builtin"
              class="dl-btn dl-btn--quiet"
              :disabled="testingHostId === h.id"
              @click="testHost(h)"
            >
              {{
                testingHostId === h.id
                  ? `测试中…`
                  : h.status
                    ? `重试`
                    : `测试连接`
              }}
            </button>
            <!-- 本机也能编辑：改名、换数据目录、registry、额外环境变量。它只是
                 这台 Mac，没有可拨的地址，所以永远没有测试连接和删除。 -->
            <button class="dl-btn dl-btn--quiet" @click="startEditHost(h)">
              {{ `编辑` }}
            </button>
            <button
              v-if="!h.builtin"
              class="dl-btn dl-btn--danger host-delete"
              @click="hostPendingDelete = h"
            >
              <DlIcon name="trash" :size="13" />
              {{ `删除` }}
            </button>
          </div>
        </header>

        <!-- An unreachable machine says why, in everyday words, and keeps the
             technical reply folded away. It is a state, not a dead record: the
             row survives and 「重试」 is the move. -->
        <div v-if="hostErrors[h.id]" class="host-error">
          <DlIcon name="alert" :size="13" class="host-error-icon" />
          <div class="host-error-body">
            <span class="host-error-reason">{{
              hostErrors[h.id]?.reason
            }}</span>
            <span class="host-error-advice">{{
              hostErrors[h.id]?.advice
            }}</span>
            <details class="host-error-detail">
              <summary>{{ `查看错误详情` }}</summary>
              <code class="dl-mono">{{ hostErrors[h.id]?.raw }}</code>
            </details>
          </div>
        </div>

        <div class="host-facts">
          <div class="host-fact">
            <span class="host-fact-label">{{ `实例` }}</span>
            <span class="host-fact-value dl-tnum">{{
              instanceCount(h.id)
            }}</span>
          </div>
          <div class="host-fact">
            <span class="host-fact-label">{{ `DSH 版本` }}</span>
            <span
              class="host-fact-value dl-tnum"
              :title="`这台机器上已安装的版本数`"
              >{{ versionsLabel(h.id) }}</span
            >
          </div>
          <div class="host-fact">
            <span class="host-fact-label">{{ `DSH_HOME` }}</span>
            <span
              class="host-fact-value dl-tnum"
              :title="`启动器记录的 DSH_HOME 数`"
              >{{ homesLabel(h.id) }}</span
            >
          </div>
          <div class="host-fact host-fact--wide">
            <span class="host-fact-label">{{ `数据目录` }}</span>
            <span
              class="host-fact-value dl-mono host-data-dir"
              :title="dataDirLabel(h)"
              >{{ dataDirLabel(h) }}</span
            >
          </div>
        </div>

        <!-- The full connection-test result for the machine that was just
             tested: the same answer 环境检测 would show, without leaving here. -->
        <div v-if="testResult && testResult.hostId === h.id" class="host-test">
          <div class="host-test-head">
            <span
              class="dl-tag"
              :class="testResult.test.ok ? 'dl-tag--run' : 'dl-tag--warn'"
            >
              {{ testResult.test.ok ? `可达` : `不可达` }}
            </span>
            <span v-if="testResult.test.message" class="host-test-msg">{{
              testResult.test.message
            }}</span>
          </div>
          <div v-if="testResult.test.ok" class="host-test-facts">
            <div
              v-for="fact in testFacts(testResult.test)"
              :key="fact.label"
              class="host-test-fact"
            >
              <span class="host-test-label">{{ fact.label }}</span>
              <span class="host-test-value dl-mono" :title="fact.value">{{
                fact.value
              }}</span>
            </div>
          </div>
        </div>

        <footer class="host-card-foot">
          <button class="dl-btn dl-btn--quiet" @click="goEnvironment(h)">
            <DlIcon name="health" :size="13" />
            {{ `环境检测` }}
          </button>
          <button class="dl-btn dl-btn--quiet" @click="goResources(h)">
            <DlIcon name="library" :size="13" />
            {{ `查看资源` }}
          </button>
        </footer>
      </article>
    </div>

    <!-- Add / edit a remote host. Nothing here is dialed: a record is a
         bookmark, and 「测试连接」 is what talks to the machine. -->
    <a-modal
      :visible="hostSheet"
      :title="hostSheetTitle"
      :ok-text="`保存`"
      :cancel-text="`取消`"
      :ok-loading="saveHostAction.busy['*']"
      unmount-on-close
      @ok="saveHost"
      @cancel="hostSheet = false"
    >
      <div class="dl-field">
        <span class="dl-field-label">{{ `显示名` }}</span>
        <a-input
          v-model="hostForm.display_name"
          :placeholder="`例如：阿里云服务器`"
        />
      </div>
      <div v-if="!editingBuiltin" class="dl-field">
        <span class="dl-field-label">{{ `ssh 目标` }}</span>
        <a-select
          v-if="sshAliases.length"
          :model-value="hostForm.ssh_target"
          :placeholder="`从 ~/.ssh/config 挑选`"
          allow-clear
          allow-search
          @change="(v) => (hostForm.ssh_target = String(v ?? ''))"
        >
          <a-option v-for="a in sshAliases" :key="a.alias" :value="a.alias">
            {{ a.alias }}
            <span v-if="aliasTarget(a)" class="alias-target">{{
              aliasTarget(a)
            }}</span>
          </a-option>
        </a-select>
        <a-input
          v-model="hostForm.ssh_target"
          class="dl-mono"
          :placeholder="`zjq_aliyun 或 user@host`"
        />
        <span class="dl-field-hint">{{
          `整串原样交给系统 ssh；~/.ssh/config 里的别名、user@host、user@host -p 2222 都可以。`
        }}</span>
        <span v-if="!sshAliases.length" class="dl-field-hint">
          {{ `~/.ssh/config 里没有可用的别名` }}
        </span>
      </div>
      <div v-else class="dl-field">
        <span class="dl-field-hint">
          {{
            `本机就是运行启动器的这台 Mac，没有 ssh 目标；这里改的是它的名字与数据目录。`
          }}
        </span>
      </div>
      <div class="dl-field">
        <span class="dl-field-label">{{ `registry（可选）` }}</span>
        <a-input
          v-model="hostForm.registry"
          :placeholder="`留空就用启动器自己的`"
        />
        <span class="dl-field-hint">{{
          `只有这台主机有自己的 registry 时才填。`
        }}</span>
      </div>
      <div class="dl-field">
        <span class="dl-field-label">{{ `额外环境变量（可选）` }}</span>
        <a-textarea
          v-model="hostForm.extra_env"
          :placeholder="`每行一个：KEY=VALUE`"
          :auto-size="{ minRows: 2, maxRows: 5 }"
        />
        <span class="dl-field-hint">{{
          `在这台主机上装版本与跑实例时都注入。`
        }}</span>
      </div>
      <div class="dl-field">
        <span class="dl-field-label">{{ `数据目录（可选）` }}</span>
        <a-input
          v-model="hostForm.data_dir"
          class="dl-mono"
          :placeholder="`留空就用那台机器上的默认目录`"
        />
        <span class="dl-field-hint">{{
          `这台主机上放版本、专属 DSH_HOME、日志与运行记录的地方；留空则按那台机器上的家目录推（~/.local/share/dsh-launcher）。写 ~/dsh 这样的路径会在那台机器上展开，第一次测连接时定下来。`
        }}</span>
      </div>
    </a-modal>

    <!-- Deleting a host deletes the launcher's records and nothing else. -->
    <a-modal
      :visible="!!hostPendingDelete"
      :title="`删除主机「${hostPendingDelete?.display_name ?? ''}」？`"
      :ok-text="`删除`"
      :cancel-text="`取消`"
      :ok-loading="removeHostAction.busy['*']"
      @ok="confirmDeleteHost"
      @cancel="hostPendingDelete = null"
    >
      <p class="delete-warning">
        {{
          `只删启动器里的这条记录：服务器上什么都不动，在那台机器上跑着的实例会继续跑。`
        }}
      </p>
      <p v-if="hostRunsOnDelete.length" class="delete-warning">
        {{
          `「${hostRunNames}」会继续在那台机器上跑，启动器里再也看不到它们——要停只能在服务器上停。`
        }}
      </p>
      <p class="delete-warning">
        {{
          `启动器里归属这台主机的版本、DSH_HOME 与实例记录会一起消失，此后它上面的实例在启动器里看不到了。`
        }}
      </p>
      <p class="delete-warning">
        {{
          `远端数据目录原样留在那台机器上（版本、专属 DSH_HOME、日志、运行记录）。想在服务器上自己看一眼，可以在终端里跑：`
        }}
      </p>
      <div class="cleanup-command">
        <code v-if="inspectCommand" class="dl-mono">{{ inspectCommand }}</code>
        <span v-else-if="inspectCommandError" class="dl-row-desc">{{
          inspectCommandError
        }}</span>
        <span v-else class="dl-row-desc">{{ `正在取回命令…` }}</span>
        <button
          class="dl-btn dl-btn--quiet"
          :disabled="!inspectCommand"
          @click="copy(inspectCommand, '查看命令已复制')"
        >
          {{ `复制` }}
        </button>
      </div>
    </a-modal>
  </div>
</template>

<style lang="scss" scoped>
.hosts-page {
  max-width: var(--dl-content);
}

.host-list {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(430px, 1fr));
  gap: 16px;
  align-items: start;
}

.host-card {
  overflow: hidden;
  padding: 16px 18px 14px;
}

.host-card-head {
  display: flex;
  align-items: flex-start;
  gap: 11px;
}

.host-dot {
  width: 8px;
  height: 8px;
  margin-top: 6px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--dl-ink-4);

  &.is-local,
  &.is-ok {
    background: var(--dl-run);
  }

  &.is-warn {
    background: var(--dl-warn);
  }

  &.is-busy {
    background: var(--dl-signal);
  }
}

.host-id {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}

.host-name-line {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.host-name {
  font-size: var(--dl-text-lg);
  font-weight: 600;
  color: var(--dl-ink);
}

.host-target {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.host-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.host-delete {
  margin-left: 4px;
}

.host-facts {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 10px 18px;
  margin-top: 13px;
  padding-top: 12px;
  border-top: 1px solid var(--dl-line);
}

.host-fact {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.host-fact--wide {
  grid-column: 1 / -1;
}

.host-fact-label {
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-4);
}

.host-fact-value {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink);
}

.host-data-dir {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.host-test {
  margin-top: 12px;
  padding: 10px 12px;
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface-2);
}

.host-test-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.host-test-msg {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.host-test-facts {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 4px 20px;
  margin-top: 8px;
}

.host-test-fact {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.host-test-label {
  width: 56px;
  flex-shrink: 0;
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-4);
}

.host-test-value {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.host-card-foot {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 12px;
}

.host-error {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  margin-top: 10px;
}

.host-error-icon {
  margin-top: 1px;
  flex-shrink: 0;
  color: var(--dl-danger);
}

.host-error-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.host-error-reason {
  font-size: var(--dl-text-sm);
  line-height: 1.45;
  font-weight: 600;
  color: var(--dl-ink);
}

.host-error-advice {
  font-size: var(--dl-text-sm);
  line-height: 1.45;
  color: var(--dl-ink-3);
}

.host-error-detail {
  margin-top: 3px;

  summary {
    width: fit-content;
    cursor: pointer;
    font-size: var(--dl-text-xs);
    font-weight: 600;
    color: var(--dl-danger);
  }

  code {
    display: block;
    margin-top: 4px;
    padding: 6px 8px;
    border-radius: var(--dl-r-ctl);
    background: var(--dl-danger-soft);
    color: var(--dl-ink-2);
    font-size: var(--dl-text-xs);
    line-height: 1.5;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
}

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
</style>

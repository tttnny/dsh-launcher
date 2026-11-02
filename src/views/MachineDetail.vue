<script setup lang="ts">
import { computed, h, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import type { ConnectionTest, HostWithStatus, RemoteTool } from '@/api/types'
import { useAction } from '@/composables/useAction'
import { useLauncherStore } from '@/stores/launcher'
import PageHeader from '@/components/PageHeader.vue'
import StatusBadge from '@/components/StatusBadge.vue'
import EmptyState from '@/components/EmptyState.vue'
import MachineFormDialog from '@/components/MachineFormDialog.vue'
import EnvironmentPanel from '@/components/EnvironmentPanel.vue'
import DlIcon from '@/components/DlIcon.vue'

// 一台机器的详情：它在哪里、连不连得上、运行环境是否就绪、带着多少资源，
// 以及怎么把这条记录删掉。删除只动启动器自己的记录，机器上的东西一样不碰。

const route = useRoute()
const router = useRouter()
const store = useLauncherStore()

const hostId = computed(() => String(route.params.id ?? ''))
const host = computed<HostWithStatus | null>(() => store.hostById(hostId.value))
const loading = ref(!store.loaded)

watch(
  () => store.loaded,
  (value) => {
    if (value) loading.value = false
  },
  { immediate: true },
)

const formVisible = ref(false)
const testing = ref(false)
const testResult = ref<ConnectionTest | null>(null)
const testError = ref('')

watch(hostId, () => {
  testResult.value = null
  testError.value = ''
})

function badgeState(value: HostWithStatus): string {
  if (value.builtin) return 'ok'
  const state = value.status?.state
  if (state === 'reachable') return 'ok'
  if (state === 'unreachable') return 'unreachable'
  if (state === 'connecting') return 'starting'
  return 'idle'
}

function badgeLabel(value: HostWithStatus): string {
  if (value.builtin) return '正常'
  const state = value.status?.state
  if (state === 'reachable') return '可达'
  if (state === 'unreachable') return '主机不可达'
  if (state === 'connecting') return '连接中'
  return '未测试'
}

const headDesc = computed(() => {
  if (!host.value) return ''
  return host.value.builtin ? '本地机器' : '远程主机'
})

const extraEnvEntries = computed(() =>
  Object.entries(host.value?.extra_env ?? {}),
)

function systemLine(test: ConnectionTest): string {
  const system = test.system
  return system
    ? system.os + ' ' + system.release + '（' + system.arch + '）'
    : '—'
}

function toolLine(tool: RemoteTool | null): string {
  if (!tool) return '没有这个命令'
  return tool.version ? tool.version + ' · ' + tool.path : tool.path
}

function dataDirLine(test: ConnectionTest): string {
  if (test.data_dir_problem) return test.data_dir_problem
  const dir = test.data_dir ?? '—'
  return test.data_dir_exists ? dir : dir + '（还不存在，第一次写入时会创建）'
}

const testFacts = computed(() => {
  const test = testResult.value
  if (!test || !test.ok) return []
  return [
    { label: '系统', value: systemLine(test) },
    { label: '家目录', value: test.home ?? '—' },
    { label: '数据目录', value: dataDirLine(test) },
    { label: 'Node', value: toolLine(test.node) },
    { label: 'pnpm', value: toolLine(test.pnpm) },
  ]
})

async function refreshHosts() {
  try {
    const list = await api.listHosts()
    store.setHosts(list)
    const value = host.value
    if (
      value &&
      ((!store.versionsByHost[value.id] && !store.versionErrors[value.id]) ||
        (!store.homesByHost[value.id] && !store.homeErrors[value.id]))
    ) {
      void store.loadHostResources(value.id)
    }
  } catch (e) {
    Message.error(String(e))
  }
}

onMounted(() => {
  if (store.loaded) void refreshHosts()
})

async function testConnection() {
  if (!host.value || host.value.builtin) return
  testing.value = true
  testError.value = ''
  try {
    testResult.value = await api.testHostConnection(host.value.id)
    await refreshHosts()
  } catch (e) {
    testError.value = String(e)
  } finally {
    testing.value = false
  }
}

// --- 资源 -------------------------------------------------------------------

const instanceCount = computed(() => {
  if (!host.value) return 0
  return store.instancesOfHost(host.value.id).length
})

const versionCount = computed(() => {
  if (!host.value) return '—'
  if (store.versionErrors[host.value.id]) return '—'
  const list = store.versionsByHost[host.value.id]
  return list ? String(list.length) : '—'
})

const homeCount = computed(() => {
  if (!host.value) return '—'
  if (store.homeErrors[host.value.id]) return '—'
  const list = store.homesByHost[host.value.id]
  return list ? String(list.length) : '—'
})

const runningNames = computed(() => {
  if (!host.value) return []
  return store
    .instancesOfHost(host.value.id)
    .filter((inst) => {
      const state = store.statusOf(inst.id).state
      return (
        state === 'running' || state === 'starting' || state === 'unreachable'
      )
    })
    .map((inst) => inst.name)
})

function goInstances() {
  if (!host.value) return
  void store.selectHost(host.value.id)
  store.setAllHosts(false)
  void router.push({ name: 'instances' })
}

function goInstall() {
  if (!host.value) return
  void store.selectHost(host.value.id)
  void router.push({ name: 'install' })
}

function goData() {
  if (!host.value) return
  void store.selectHost(host.value.id)
  void router.push({ name: 'data' })
}

// --- 编辑 / 删除 -------------------------------------------------------------

function editHost() {
  formVisible.value = true
}

const removeHostAction = useAction(
  async (id: string) => {
    await api.deleteHost(id)
    return true
  },
  { success: () => '主机记录已删除' },
)

function askDelete() {
  const value = host.value
  if (!value) return
  const lines = [
    '只删除启动器里的这条记录：机器上的版本、数据目录和正在运行的进程都不受影响。',
  ]
  if (runningNames.value.length) {
    lines.push(
      '仍在那台机器上运行：' +
        runningNames.value.join('、') +
        '。启动器里将看不到它们，要停只能在服务器上停。',
    )
  }
  lines.push('启动器里归属这台机器的版本与数据目录记录会一起消失。')

  Modal.confirm({
    title: '删除主机「' + value.display_name + '」？',
    content: () =>
      h(
        'div',
        null,
        lines.map((line) => h('p', { style: 'margin:0 0 8px' }, line)),
      ),
    okText: '删除',
    cancelText: '取消',
    okButtonProps: { status: 'danger' },
    onOk: async () => {
      const ok = await removeHostAction.run(value.id)
      if (ok !== true) return
      void router.push({ name: 'machines' })
    },
  })
}
</script>

<template>
  <div v-if="!store.loaded" class="page">
    <PageHeader title="机器" back-to="/machines" back-label="返回机器" />
    <div class="surface surface--pad reading">
      <span class="spinner" />
      正在读取机器信息…
    </div>
  </div>

  <div v-else-if="!host" class="page">
    <PageHeader title="机器" back-to="/machines" back-label="返回机器" />
    <div class="surface">
      <EmptyState
        icon="machine"
        title="找不到这台机器"
        description="它可能已经被删除了。"
      >
        <button
          class="btn btn--primary"
          @click="router.push({ name: 'machines' })"
        >
          返回机器列表
        </button>
      </EmptyState>
    </div>
  </div>

  <div v-else class="page">
    <PageHeader
      :title="host.display_name"
      :description="headDesc"
      back-to="/machines"
      back-label="返回机器"
    >
      <template #actions>
        <StatusBadge :state="badgeState(host)" :label="badgeLabel(host)" />
        <button
          v-if="!host.builtin"
          class="btn btn--quiet"
          :disabled="testing"
          @click="testConnection"
        >
          <DlIcon name="refresh" :size="13" />
          {{ testing ? '测试中…' : '测试连接' }}
        </button>
        <button class="btn btn--quiet" @click="editHost">编辑</button>
        <button v-if="!host.builtin" class="btn btn--danger" @click="askDelete">
          <DlIcon name="trash" :size="13" />
          删除
        </button>
      </template>
    </PageHeader>

    <section class="section">
      <div class="section-head">
        <span class="section-title">连接</span>
        <span v-if="host.builtin" class="section-hint"
          >本机就在这台 Mac 上，不需要测试连接。</span
        >
      </div>

      <div class="surface">
        <div class="field-row">
          <div class="field-row-info">
            <span class="field-label">位置</span>
          </div>
          <span v-if="host.builtin" class="conn-value">这台 Mac</span>
          <span v-else class="conn-value mono" :title="host.ssh_target">{{
            host.ssh_target
          }}</span>
        </div>
        <div class="field-row">
          <div class="field-row-info">
            <span class="field-label">数据目录</span>
            <span class="field-hint"
              >启动器在这台机器上放版本、日志与运行记录的地方。</span
            >
          </div>
          <span class="conn-value mono">{{ host.data_dir || '默认目录' }}</span>
        </div>
        <div class="field-row">
          <div class="field-row-info">
            <span class="field-label">registry</span>
          </div>
          <span class="conn-value mono">{{
            host.registry || '跟随启动器默认'
          }}</span>
        </div>
        <div class="field-row">
          <div class="field-row-info">
            <span class="field-label">额外环境变量</span>
            <span class="field-hint">在这台机器上装版本与跑实例时都注入。</span>
          </div>
          <div v-if="extraEnvEntries.length" class="conn-env">
            <span
              v-for="[key, value] in extraEnvEntries"
              :key="key"
              class="conn-env-item mono"
              >{{ key }}={{ value }}</span
            >
          </div>
          <span v-else class="conn-value muted">无</span>
        </div>
      </div>

      <div v-if="testError" class="notice notice--danger conn-result">
        <div class="notice-body">
          <div class="notice-title">测试连接失败</div>
          <div>{{ testError }}</div>
        </div>
      </div>

      <div v-else-if="testResult" class="surface surface--pad conn-result">
        <div class="test-head">
          <StatusBadge
            :state="testResult.ok ? 'ok' : 'unreachable'"
            :label="testResult.ok ? '可达' : '不可达'"
          />
          <span v-if="testResult.message" class="test-message">{{
            testResult.message
          }}</span>
        </div>
        <div v-if="testResult.ok" class="test-facts">
          <div v-for="fact in testFacts" :key="fact.label" class="test-fact">
            <span class="test-label">{{ fact.label }}</span>
            <span class="test-value mono">{{ fact.value }}</span>
          </div>
        </div>
      </div>

      <div
        v-else-if="!host.builtin && host.status?.state === 'unreachable'"
        class="notice notice--warn conn-result"
      >
        <div class="notice-body">
          <div class="notice-title">这台机器现在连不上</div>
          <div>{{ host.status.message }}</div>
        </div>
        <button
          class="btn btn--quiet btn--sm"
          :disabled="testing"
          @click="testConnection"
        >
          重试
        </button>
      </div>
    </section>

    <EnvironmentPanel :host-id="hostId" />

    <section class="section">
      <div class="section-head">
        <span class="section-title">资源</span>
        <span class="section-hint">点击进入对应的页面查看。</span>
      </div>
      <div class="surface resource-grid">
        <button class="resource-card" @click="goInstances">
          <span class="resource-num tnum">{{ instanceCount }}</span>
          <span class="resource-label">实例</span>
        </button>
        <button class="resource-card" @click="goInstall">
          <span class="resource-num tnum">{{ versionCount }}</span>
          <span class="resource-label">版本</span>
        </button>
        <button class="resource-card" @click="goData">
          <span class="resource-num tnum">{{ homeCount }}</span>
          <span class="resource-label">数据目录</span>
        </button>
      </div>
    </section>

    <section v-if="!host.builtin" class="section">
      <div class="section-head">
        <span class="section-title">删除</span>
      </div>
      <div class="surface surface--pad danger-zone">
        <div class="danger-main">
          <span class="field-label">删除这台机器</span>
          <span class="field-hint"
            >只删除启动器里的这条记录。机器上的版本、数据目录和正在运行的进程都不受影响。</span
          >
          <span v-if="runningNames.length" class="danger-run"
            >仍在那台机器上运行：{{ runningNames.join('、') }}</span
          >
        </div>
        <button class="btn btn--danger-solid" @click="askDelete">
          <DlIcon name="trash" :size="13" />
          删除机器
        </button>
      </div>
    </section>

    <MachineFormDialog
      v-model:visible="formVisible"
      :host="host"
      @saved="refreshHosts"
    />
  </div>
</template>

<style lang="scss" scoped>
.reading {
  display: flex;
  align-items: center;
  gap: 9px;
  color: var(--dl-ink-3);
}

.conn-value {
  min-width: 0;
  max-width: 62ch;
  overflow-wrap: anywhere;
  color: var(--dl-ink-2);
  font-size: var(--dl-text-md);
}

.conn-env {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
  text-align: right;
}

.conn-env-item {
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
  overflow-wrap: anywhere;
}

.conn-result {
  margin-top: 12px;
}

.test-head {
  display: flex;
  align-items: center;
  gap: 9px;
}

.test-message {
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
  line-height: 1.6;
}

.test-facts {
  display: grid;
  gap: 8px;
  margin-top: 10px;
}

.test-fact {
  display: flex;
  align-items: baseline;
  gap: 12px;
  min-width: 0;
}

.test-label {
  width: 62px;
  flex-shrink: 0;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.test-value {
  min-width: 0;
  overflow-wrap: anywhere;
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
}

.resource-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
}

.resource-card {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 3px;
  padding: 16px 18px;
  border: none;
  background: transparent;
  color: var(--dl-ink);
  font-family: inherit;
  text-align: left;
  cursor: pointer;

  & + & {
    border-left: 1px solid var(--dl-line);
  }

  &:hover {
    background: var(--dl-surface-2);
  }
}

.resource-num {
  font-family: var(--dl-font-display);
  font-size: var(--dl-text-2xl);
  font-weight: 600;
  line-height: 1.1;
}

.resource-label {
  color: var(--dl-ink-2);
  font-size: var(--dl-text-md);
  font-weight: 550;
}

.resource-go {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  margin-top: 4px;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.danger-zone {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  border-color: rgba(174, 59, 52, 0.28);
}

.danger-main {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.danger-run {
  color: var(--dl-danger);
  font-size: var(--dl-text-sm);
  line-height: 1.5;
}
</style>

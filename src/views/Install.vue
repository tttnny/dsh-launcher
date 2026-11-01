<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { installKey, useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import type { DshVersion, RemoteVersion, TaskInfo } from '@/api/types'
import PageHeader from '@/components/PageHeader.vue'
import EmptyState from '@/components/EmptyState.vue'
import DlIcon from '@/components/DlIcon.vue'
import DlRefreshButton from '@/components/DlRefreshButton.vue'

// 安装页只回答一件事：在这台机器上装哪个版本。左边是已经装好的，右边是还能装的；
// 一次安装的进度就长在它自己的版本行上，装完自动从可安装区移到已安装区。

const store = useLauncherStore()

const collator = new Intl.Collator('en', { numeric: true })
const isPrerelease = (v: string) => v.includes('-')

const currentHostLabel = computed(() => {
  const host = store.currentHost
  return host?.builtin ? '本机' : (host?.display_name ?? store.currentHostId)
})

// --- 读取状态 ---------------------------------------------------------------

const pendingInstalled = ref(true)
const refreshingInstalled = ref(false)
const switchingHost = ref(false)

const installedLoading = computed(
  () =>
    pendingInstalled.value || switchingHost.value || refreshingInstalled.value,
)

async function refreshInstalled() {
  refreshingInstalled.value = true
  try {
    await store.loadHostResources(store.currentHostId)
  } finally {
    refreshingInstalled.value = false
  }
}

async function pickHost(id: string) {
  if (id === store.currentHostId) return
  switchingHost.value = true
  try {
    await store.selectHost(id)
  } finally {
    switchingHost.value = false
  }
}

onMounted(async () => {
  // 可安装列表自己带 loading 标记，先发出去，已安装列表单独等它自己的读取。
  void store.refreshRemoteVersions()
  void store.refreshInstallTasks()
  try {
    await store.loadHostResources(store.currentHostId)
  } finally {
    pendingInstalled.value = false
  }
})

// --- 已安装 -----------------------------------------------------------------

const installedRows = computed(() =>
  [...store.versions]
    .sort((a, b) => collator.compare(b.version, a.version))
    .map((v) => ({ ...v, used: store.instancesOfVersion(v.id).length })),
)

// --- 可安装 -----------------------------------------------------------------

const installedSet = computed(
  () => new Set(store.versions.map((v) => v.version)),
)

const installable = computed(() =>
  [...store.remoteVersions]
    .sort((a, b) => collator.compare(b.version, a.version))
    .filter((v) => !installedSet.value.has(v.version)),
)

type Channel = 'all' | 'stable' | 'prerelease'
const channel = ref<Channel>('all')
const search = ref('')

const visibleInstallable = computed(() => {
  const q = search.value.trim().toLowerCase()
  const inChannel = installable.value.filter((v) => {
    if (channel.value === 'stable') return !isPrerelease(v.version)
    if (channel.value === 'prerelease') return isPrerelease(v.version)
    return true
  })
  return q
    ? inChannel.filter((v) => v.version.toLowerCase().includes(q))
    : inChannel
})

function installOf(version: string): TaskInfo | null {
  return (
    store.installingVersions[installKey(store.currentHostId, version)] ?? null
  )
}

const installableRows = computed(() =>
  visibleInstallable.value.map((v) => ({ ...v, task: installOf(v.version) })),
)

const emptyTitle = computed(() =>
  installable.value.length === 0 ? '所有版本都已安装' : '没有匹配的版本',
)
const emptyDesc = computed(() =>
  installable.value.length === 0
    ? '这台机器上已经没有可再安装的版本。'
    : '换一个关键词或筛选条件再试。',
)

// --- 安装 -------------------------------------------------------------------

const installAction = useAction(
  async (version: string) => {
    const hostId = store.currentHostId
    const host = currentHostLabel.value
    const taskId = await api.startInstallVersionTask(version, hostId)
    return { taskId, hostId, version, host }
  },
  {
    key: (version) => version,
    success: ({ version, host }) => `已开始在「${host}」上安装 DSH ${version}`,
  },
)

function onInstall(row: RemoteVersion) {
  if (installedSet.value.has(row.version)) {
    Message.info(`版本 ${row.version} 已经安装`)
    return
  }
  const fromSource = row.source === 'github'
  Modal.confirm({
    title: `安装版本并创建实例（${currentHostLabel.value}）`,
    content: fromSource
      ? `在「${currentHostLabel.value}」上安装 DSH ${row.version}？该版本只在 GitHub 发布，会在那台机器上从源码克隆并构建（可能较慢），完成后自动创建实例和专属数据目录。`
      : `在「${currentHostLabel.value}」上下载并安装 DSH ${row.version}？完成后会在那台机器上创建对应实例和专属数据目录。`,
    okText: '安装',
    cancelText: '取消',
    onOk: async () => {
      const started = await installAction.run(row.version)
      if (started === undefined) return
      await store.watchInstallTask(started.taskId, started.hostId, row.version)
    },
  })
}

const cancelAction = useAction((id: string) => api.cancelTask(id), {
  key: (id) => id,
  success: () => '已取消安装',
})

async function onCancel(task: TaskInfo) {
  await cancelAction.run(task.id)
  await store.refreshInstallTasks()
}

const openLogs = ref<Record<string, boolean>>({})
function toggleLogs(version: string) {
  openLogs.value[version] = !openLogs.value[version]
}

const LOG_TAIL = 12
function recentLogs(task: TaskInfo): string {
  const lines = task.logs.slice(-LOG_TAIL)
  return lines.length ? lines.join('\n') : '（还没有日志）'
}

function latestLog(task: TaskInfo): string {
  return task.logs[task.logs.length - 1] ?? task.message ?? '正在准备…'
}

// --- 删除 -------------------------------------------------------------------

const removeAction = useAction(
  async (id: string, version: string) => {
    const hostId = store.currentHostId
    await api.removeVersion(id, hostId)
    await store.refreshVersions(hostId)
    await store.refreshInstances()
    await store.refreshHomes(hostId)
    return version
  },
  {
    key: (id) => id,
    success: (version) => `版本 ${version} 已删除`,
  },
)

function removeContent(v: DshVersion): string {
  const names = store.instancesOfVersion(v.id).map((i) => i.name)
  const affected = names.length
    ? `实例「${names.join('」「')}」会一并移除，`
    : ''
  return `在「${currentHostLabel.value}」上删除版本 ${v.version}？${affected}这个版本专属的 DSH_HOME「${v.dedicated_home}」也会一起删除，其中包含会话历史和凭据，删除后无法恢复。`
}

function onRemove(v: DshVersion) {
  Modal.confirm({
    title: `删除版本 ${v.version}`,
    content: removeContent(v),
    okText: '删除',
    cancelText: '取消',
    okButtonProps: { status: 'danger' },
    onOk: () => removeAction.run(v.id, v.version),
  })
}

// --- dsh 命令 ---------------------------------------------------------------

const pathLinkAction = useAction(
  async (v: DshVersion) => {
    const linked = v.path_linked
    const hostId = store.currentHostId
    await api.setVersionPathLink(linked ? null : v.id, hostId)
    await store.refreshVersions(hostId)
    return { linked, version: v.version }
  },
  {
    key: (v) => v.id,
    success: ({ linked, version }) =>
      linked
        ? `已取消版本 ${version} 的 dsh 命令`
        : `已把版本 ${version} 设为 dsh 命令`,
  },
)

function onTogglePathLink(v: DshVersion) {
  const current = store.versions.find((x) => x.path_linked && x.id !== v.id)
  if (v.path_linked || !current) {
    void pathLinkAction.run(v)
    return
  }
  Modal.confirm({
    title: `把 dsh 命令切换到 ${v.version}`,
    content: `「${currentHostLabel.value}」上的 dsh 命令现在指向版本 ${current.version}。切换后，终端里的 dsh 会改用 ${v.version}，版本 ${current.version} 不再作为命令使用。`,
    okText: `设为 ${v.version}`,
    cancelText: '取消',
    onOk: () => pathLinkAction.run(v),
  })
}
</script>

<template>
  <div class="page">
    <PageHeader
      title="安装 DSH"
      description="安装后会创建该版本专用的数据目录和一个实例。"
    />

    <!-- 机器 chips：安装永远发生在一台具体的机器上。 -->
    <div class="host-bar">
      <button
        v-for="host in store.hosts"
        :key="host.id"
        class="chip"
        :class="{ 'is-active': host.id === store.currentHostId }"
        @click="pickHost(host.id)"
      >
        {{ host.builtin ? '本机' : host.display_name }}
      </button>
    </div>

    <!-- 已安装 -->
    <section class="section">
      <div class="section-head">
        <span class="section-title">已安装</span>
        <span v-if="installedRows.length" class="section-hint tnum">
          {{ installedRows.length }} 个版本
        </span>
        <span class="section-rule" />
        <div class="section-actions">
          <DlRefreshButton
            class="icon-btn"
            title="刷新已安装版本"
            :action="refreshInstalled"
          />
        </div>
      </div>

      <div v-if="installedLoading" class="surface loading-state">
        <span class="spinner" />
        <span>正在读取这台机器的版本…</span>
      </div>

      <div v-else-if="store.versionError" class="notice notice--danger">
        <div class="notice-body">
          <div class="notice-title">读不到这台机器的已安装版本</div>
          <div>{{ store.versionError }}</div>
        </div>
        <button class="btn btn--quiet btn--sm" @click="refreshInstalled">
          重试
        </button>
      </div>

      <div v-else-if="!installedRows.length" class="surface">
        <EmptyState
          icon="version"
          title="还没有安装任何版本"
          description="从下面的「可安装」里挑一个版本安装，装好就能创建实例。"
        />
      </div>

      <div v-else class="surface">
        <div v-for="v in installedRows" :key="v.id" class="list-row">
          <div class="row-main">
            <div class="row-title">
              <span class="mono version-name">{{ v.version }}</span>
              <span
                v-if="!v.bin_ready"
                class="tag tag--warn"
                title="这个版本的启动文件不在，重新安装一次才算装好。"
              >
                安装不完整
              </span>
              <span
                v-if="v.path_linked"
                class="tag tag--accent"
                title="终端里的 dsh 命令指向这个版本。"
              >
                当前 dsh 命令
              </span>
            </div>
          </div>
          <div class="row-actions">
            <button
              class="btn btn--quiet btn--sm"
              :disabled="
                (!v.path_linked && !v.bin_ready) || !!pathLinkAction.busy[v.id]
              "
              @click="onTogglePathLink(v)"
            >
              {{ v.path_linked ? '取消 dsh 命令' : '设为 dsh 命令' }}
            </button>
            <button
              class="btn btn--danger btn--sm"
              :disabled="!!removeAction.busy[v.id]"
              @click="onRemove(v)"
            >
              <DlIcon name="trash" :size="13" />
              删除
            </button>
          </div>
        </div>
      </div>
    </section>

    <!-- 可安装 -->
    <section class="section">
      <div class="section-head">
        <span class="section-title">可安装</span>
        <span class="section-rule" />
        <div class="section-actions avail-toolbar">
          <div class="avail-filters">
            <button
              class="chip"
              :class="{ 'is-active': channel === 'all' }"
              @click="channel = 'all'"
            >
              全部
            </button>
            <button
              class="chip"
              :class="{ 'is-active': channel === 'stable' }"
              @click="channel = 'stable'"
            >
              正式版
            </button>
            <button
              class="chip"
              :class="{ 'is-active': channel === 'prerelease' }"
              @click="channel = 'prerelease'"
            >
              预览版
            </button>
          </div>
          <input
            v-model="search"
            class="input input--mono avail-search"
            placeholder="搜索版本号"
          />
          <DlRefreshButton
            class="btn btn--quiet"
            :disabled="store.remoteLoading"
            :action="() => store.refreshRemoteVersions(true)"
          >
            刷新
          </DlRefreshButton>
        </div>
      </div>

      <div v-if="store.remoteError" class="notice notice--danger">
        <div class="notice-body">
          <div class="notice-title">读不到可安装的版本</div>
          <div>{{ store.remoteError }}</div>
        </div>
        <button
          class="btn btn--quiet btn--sm"
          @click="store.refreshRemoteVersions(true)"
        >
          重试
        </button>
      </div>

      <div
        v-else-if="store.remoteLoading && !installable.length"
        class="surface loading-state"
      >
        <span class="spinner" />
        <span>正在向这台机器查询可安装的版本…</span>
      </div>

      <div v-else-if="!installableRows.length" class="surface">
        <EmptyState
          icon="search"
          :title="emptyTitle"
          :description="emptyDesc"
        />
      </div>

      <div v-else class="surface">
        <div
          v-for="row in installableRows"
          :key="row.version"
          class="install-item"
        >
          <div class="list-row">
            <div class="row-main">
              <div class="row-title">
                <span class="mono version-name">{{ row.version }}</span>
                <span v-if="isPrerelease(row.version)" class="tag tag--accent"
                  >预览版</span
                >
                <span v-if="row.source === 'github'" class="tag tag--muted"
                  >源码构建</span
                >
              </div>
            </div>
            <div class="row-actions">
              <span v-if="row.task?.state === 'running'" class="tag tag--warn"
                >安装中</span
              >
              <button
                v-else
                class="btn btn--quiet"
                :disabled="!!installAction.busy[row.version]"
                @click="onInstall(row)"
              >
                <DlIcon name="download" :size="13" />
                安装
              </button>
            </div>
          </div>

          <div
            v-if="
              row.task &&
              (row.task.state === 'running' ||
                row.task.state === 'error' ||
                row.task.state === 'cancelled')
            "
            class="install-block"
          >
            <template v-if="row.task.state === 'running'">
              <div class="install-head">
                <span class="spinner" />
                <span>正在安装…</span>
                <span class="install-percent tnum"
                  >{{ row.task.percent }}%</span
                >
                <span class="spacer" />
                <button
                  class="btn btn--danger btn--sm"
                  :disabled="!!cancelAction.busy[row.task.id]"
                  @click="onCancel(row.task)"
                >
                  取消安装
                </button>
              </div>
              <div class="track">
                <div
                  class="fill"
                  :style="{ width: Math.max(3, row.task.percent) + '%' }"
                />
              </div>
              <div class="latest-log mono">{{ latestLog(row.task) }}</div>
              <div class="install-log-actions">
                <button
                  class="btn btn--ghost btn--sm"
                  @click="toggleLogs(row.version)"
                >
                  {{ openLogs[row.version] ? '收起日志' : '查看日志' }}
                </button>
              </div>
              <pre v-if="openLogs[row.version]" class="install-log mono">{{
                recentLogs(row.task)
              }}</pre>
            </template>
            <div v-else class="notice notice--danger">
              <div class="notice-body">
                <div class="notice-title">
                  {{
                    row.task.state === 'cancelled' ? '安装已取消' : '安装失败'
                  }}
                </div>
                <div>{{ row.task.message || '可以重试一次。' }}</div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>

<style lang="scss" scoped>
.host-bar {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 2px;
  margin-bottom: 16px;
  padding: 3px;
  border-radius: 8px;
  background: var(--dl-surface-3);
}

.version-name {
  font-size: var(--dl-text-lg);
  font-weight: 600;
  letter-spacing: 0;
}

.path-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 44ch;
}

.path-copy {
  width: 22px;
  height: 22px;
}

.meta-sep {
  color: var(--dl-ink-4);
}

.avail-toolbar {
  flex-wrap: wrap;
}

.avail-filters {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 3px;
  border-radius: 8px;
  background: var(--dl-surface-3);
}

.avail-search {
  width: 172px;
}

.install-item + .install-item {
  border-top: 1px solid var(--dl-line);
}

.install-block {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 0 16px 14px 16px;
}

.install-head {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
}

.install-percent {
  margin-left: 2px;
  color: var(--dl-ink);
  font-weight: 600;
}

.track {
  height: 6px;
  border-radius: 999px;
  overflow: hidden;
  background: var(--dl-surface-3);
}

.fill {
  height: 100%;
  border-radius: 999px;
  background: var(--dl-run);
  transition: width var(--dl-slow) var(--dl-ease);
}

.latest-log {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.install-log-actions {
  display: flex;
}

.install-log {
  margin: 0;
  max-height: 180px;
  overflow: auto;
  padding: 10px 12px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface-2);
  color: var(--dl-ink-2);
  font-size: var(--dl-text-xs);
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
}

.loading-state {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 26px 16px;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-md);
}
</style>

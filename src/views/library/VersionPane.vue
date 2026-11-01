<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { installKey, useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import type { DshVersion, RemoteVersion, TaskInfo } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'
import DlEmpty from '@/components/DlEmpty.vue'
import DlRefreshButton from '@/components/DlRefreshButton.vue'

// Installing a version is the app's only long task, and its outcome is a
// version directory plus a 1:1 instance — so the progress lives on the version
// it installs, on the machine it installs onto, and there is no task page to
// visit.

const store = useLauncherStore()

onMounted(() => {
  void store.refreshRemoteVersions()
  void store.refreshInstallTasks()
})

const collator = new Intl.Collator('en', { numeric: true })
const sortedDesc = computed(() =>
  [...store.remoteVersions].sort((a, b) =>
    collator.compare(b.version, a.version),
  ),
)
const isPrerelease = (v: string) => v.includes('-')

const installedSet = computed(
  () => new Set(store.versions.map((v) => v.version)),
)
const installedVersions = computed(() =>
  [...store.versions].sort((a, b) => collator.compare(b.version, a.version)),
)

/** The machine an install goes onto: the Library's current host. */
const hostLabel = computed(() => {
  const host = store.currentHost
  return host?.builtin ? `本机` : (host?.display_name ?? store.currentHostId)
})

// Remote versions split into what can be installed and what already is.
const installable = computed(() =>
  sortedDesc.value.filter((v) => !installedSet.value.has(v.version)),
)

type Channel = 'all' | 'stable' | 'prerelease'
const channel = ref<Channel>('all')
const search = ref('')

const visibleInstallable = computed(() => {
  const q = search.value.trim().toLowerCase()
  const inChannel = (() => {
    switch (channel.value) {
      case 'stable':
        return installable.value.filter((v) => !isPrerelease(v.version))
      case 'prerelease':
        return installable.value.filter((v) => isPrerelease(v.version))
      default:
        return installable.value
    }
  })()
  return q
    ? inChannel.filter((v) => v.version.toLowerCase().includes(q))
    : inChannel
})

/** The search box is only worth its chrome once the list stops fitting. */
const searchable = computed(() => installable.value.length > 8)

// --- Install ------------------------------------------------------------------

const installAction = useAction(
  async (version: string) => {
    // The install runs on the machine the Library is showing, and the row it
    // shows its progress on is keyed by exactly that host. Both the id and the
    // name come from the click, so a host switch while the call is in flight
    // cannot relabel where the install went.
    const hostId = store.currentHostId
    const host = hostLabel.value
    const taskId = await api.startInstallVersionTask(version, hostId)
    return { taskId, hostId, version, host }
  },
  {
    key: (version) => version,
    success: ({ version, host }) => `已开始在「${host}」上安装 DSH ${version}`,
  },
)

/** The install in flight (or the last outcome) of one version **on the host
 *  this page is showing**. Two machines can install the same version at once:
 *  each of them has its own row here. */
function installOf(version: string): TaskInfo | null {
  return (
    store.installingVersions[installKey(store.currentHostId, version)] ?? null
  )
}

const isInstalling = (version: string) =>
  installOf(version)?.state === 'running'

/** The tail of an install's log: the progress bar says how far along it is,
 *  these lines say what it is doing. */
const LOG_TAIL = 8
function recentLogs(task: TaskInfo): string[] {
  return task.logs.slice(-LOG_TAIL)
}

function onInstall(row: RemoteVersion) {
  if (installedSet.value.has(row.version)) {
    Message.info(`版本 ${row.version} 已经安装`)
    return
  }
  const isSourceBuild = row.source === 'github'
  Modal.confirm({
    title: `安装版本并创建实例（${hostLabel.value}）`,
    content: isSourceBuild
      ? `在「${hostLabel.value}」上安装 DSH ${row.version}？该版本只在 GitHub 发布，会在那台机器上从源码克隆并构建（可能较慢），完成后自动创建实例与专属目录。`
      : `在「${hostLabel.value}」上下载并安装 DSH ${row.version}？完成后会在那台机器上创建对应实例与专属目录。`,
    okText: `安装`,
    cancelText: `取消`,
    modalClass: 'dl-sheet',
    onOk: async () => {
      const started = await installAction.run(row.version)
      if (started === undefined) return
      await store.watchInstallTask(started.taskId, started.hostId, row.version)
    },
  })
}

const cancelAction = useAction((id: string) => api.cancelTask(id), {
  key: (id) => id,
})

async function onCancelInstall(taskId: string) {
  await cancelAction.run(taskId)
  await store.refreshInstallTasks()
}

// --- Remove -------------------------------------------------------------------

const removeAction = useAction(
  async (id: string, version: string) => {
    await api.removeVersion(id, store.currentHostId)
    await store.refreshVersions()
    await store.refreshInstances()
    await store.refreshHomes()
    return version
  },
  {
    key: (id) => id,
    success: (version) => `版本 ${version} 已删除`,
  },
)

const usedByCount = (versionId: string) =>
  store.instancesOfVersion(versionId).length

// --- PATH registration ---------------------------------------------------------

/** The link is `~/.local/bin/dsh` on the machine this page is showing — 本机
 *  or a server — and the backend writes it wherever that is. */
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
        ? `版本 ${version} 的 dsh 登记已取消`
        : `版本 ${version} 已登记为 dsh`,
  },
)
</script>

<template>
  <!-- Installed first: the versions you have are the ones you act on. A host
       that could not be reached has **no** list here — the previous one is not
       this host's answer (ADR-0006), so the section says what the machine said
       instead of showing it. -->
  <section class="dl-section">
    <div class="dl-section-label">
      {{ `已安装` }}
      <span class="ver-label-spacer" />
      <DlRefreshButton
        class="dl-icon-btn"
        :title="`刷新`"
        :action="() => store.loadHostResources(store.currentHostId)"
      />
    </div>

    <div v-if="store.versionError" class="dl-surface dl-failure">
      <p class="dl-failure-title">{{ `读不到这台主机的资源` }}</p>
      <p class="dl-failure-text">{{ store.versionError }}</p>
      <p class="dl-failure-hint">
        {{ `上面是那台机器回的话。修好之后再试一次。` }}
      </p>
    </div>

    <div v-else-if="installedVersions.length === 0" class="dl-surface">
      <DlEmpty icon="version" :description="`还没有安装任何 DSH 版本`" />
    </div>

    <div v-else class="dl-surface">
      <div v-for="v in installedVersions" :key="v.id" class="ver-row">
        <span class="ver-mark"><DlIcon name="check" :size="14" /></span>
        <div class="ver-main">
          <div class="ver-title">
            <span class="ver-name dl-mono">{{ v.version }}</span>
            <span
              v-if="!v.bin_ready"
              class="dl-tag dl-tag--warn"
              :title="`这个版本的启动文件不在，装一次才算装好。`"
            >
              {{ `安装不完整` }}
            </span>
            <span
              v-if="v.path_linked"
              class="dl-tag dl-tag--muted"
              :title="`在版本所在主机的 ~/.local/bin 建立 dsh 命令，让 magpie、终端等按命令名查找 dsh 的程序找到这个版本。同一台机器上同一时间只能登记一个版本。`"
            >
              {{ `已在 PATH` }}
            </span>
          </div>
          <span class="ver-path dl-mono" :title="v.dir">{{ v.dir }}</span>
        </div>
        <span class="ver-used dl-tnum">{{
          `${usedByCount(v.id)} 个实例`
        }}</span>
        <button
          class="dl-btn dl-btn--quiet"
          :title="`在版本所在主机的 ~/.local/bin 建立 dsh 命令，让 magpie、终端等按命令名查找 dsh 的程序找到这个版本。同一台机器上同一时间只能登记一个版本。`"
          :disabled="
            (!v.path_linked && !v.bin_ready) || !!pathLinkAction.busy[v.id]
          "
          @click="pathLinkAction.run(v)"
        >
          {{ v.path_linked ? `取消登记` : `登记 dsh` }}
        </button>
        <a-popconfirm
          :content="`删除版本 ${v.version}？对应的实例会一并移除，专属 DSH_HOME「${v.dedicated_home}」也会被删除——其中包含会话历史与凭据，删除后不可恢复。`"
          :ok-text="`删除`"
          :cancel-text="`取消`"
          modal-class="dl-sheet"
          @ok="removeAction.run(v.id, v.version)"
        >
          <button
            class="dl-btn dl-btn--danger"
            :disabled="!!removeAction.busy[v.id]"
          >
            <DlIcon name="trash" :size="13" />
            {{ `删除` }}
          </button>
        </a-popconfirm>
      </div>
    </div>
  </section>

  <!-- Available: one filter row, then the versions that can still be installed.
       An install in flight renders its progress on its own row. -->
  <section class="dl-section">
    <div class="dl-section-label">
      {{ `可安装` }}
      <span class="ver-label-spacer" />
      <div v-if="searchable" class="dl-input dl-search ver-search">
        <DlIcon name="search" :size="13" />
        <input
          v-model="search"
          class="dl-search-input"
          :placeholder="`搜索版本号`"
        />
        <button
          v-if="search"
          class="dl-icon-btn dl-search-clear"
          @click="search = ''"
        >
          <DlIcon name="close" :size="11" />
        </button>
      </div>
      <div class="dl-segmented ver-channels">
        <button
          :class="{ 'is-active': channel === 'all' }"
          @click="channel = 'all'"
        >
          {{ `全部` }}
        </button>
        <button
          :class="{ 'is-active': channel === 'stable' }"
          @click="channel = 'stable'"
        >
          {{ `正式版` }}
        </button>
        <button
          :class="{ 'is-active': channel === 'prerelease' }"
          @click="channel = 'prerelease'"
        >
          {{ `预览版` }}
        </button>
      </div>
      <DlRefreshButton
        class="dl-icon-btn"
        :title="`刷新`"
        :disabled="store.remoteLoading"
        :action="() => store.refreshRemoteVersions(true)"
      />
    </div>

    <div class="dl-surface">
      <!-- A failed query shows what the machine said, never the previous list:
           the installable set is a fact about that host's registry. -->
      <div v-if="store.remoteError" class="dl-row-none">
        {{ store.remoteError }}
      </div>

      <div v-else-if="visibleInstallable.length === 0" class="dl-row-none">
        {{
          store.remoteLoading
            ? `加载中…`
            : search
              ? `没有匹配的版本`
              : `暂无数据`
        }}
      </div>

      <!-- The list is that host's, and an install runs **there**: the button
           installs onto the machine this page is showing. -->
      <div v-for="v in visibleInstallable" :key="v.version" class="ver-row">
        <span class="ver-mark is-idle"
          ><DlIcon name="download" :size="14"
        /></span>
        <div class="ver-main">
          <div class="ver-title">
            <span class="ver-name dl-mono">{{ v.version }}</span>
            <span v-if="isPrerelease(v.version)" class="dl-tag dl-tag--muted">
              {{ `预览版` }}
            </span>
            <span v-if="v.source === 'github'" class="dl-tag dl-tag--warn">
              {{ `源码构建` }}
            </span>
          </div>

          <!-- An install in flight: its progress, what it is doing, and a way
               out. A failure or a cancellation keeps the row — the reason the
               machine gave is the only place the user can read it, and the
               install button beside it is how they try again. -->
          <div v-if="installOf(v.version)" class="ver-install-state">
            <div v-if="isInstalling(v.version)" class="ver-progress">
              <a-progress
                :percent="(installOf(v.version)?.percent ?? 0) / 100"
                size="small"
                :show-text="true"
              />
              <button
                class="dl-btn dl-btn--quiet"
                :disabled="!!cancelAction.busy[installOf(v.version)?.id ?? '']"
                @click="onCancelInstall(installOf(v.version)!.id)"
              >
                {{ `取消安装` }}
              </button>
            </div>
            <p
              v-else-if="installOf(v.version)?.state === 'error'"
              class="ver-failed"
            >
              {{ installOf(v.version)?.message }}
            </p>
            <p
              v-else-if="installOf(v.version)?.state === 'cancelled'"
              class="ver-cancelled"
            >
              {{ `已取消，那台机器上的安装进程已停止` }}
            </p>
            <div v-if="installOf(v.version)?.logs.length" class="ver-log">
              <span
                v-for="(line, index) in recentLogs(installOf(v.version)!)"
                :key="index"
                class="ver-log-line dl-mono"
                >{{ line }}</span
              >
            </div>
          </div>
        </div>

        <button
          v-if="!isInstalling(v.version)"
          class="dl-btn dl-btn--quiet ver-install"
          :disabled="!!installAction.busy[v.version]"
          @click="onInstall(v)"
        >
          {{ installOf(v.version)?.state === 'error' ? '重试' : '安装' }}
        </button>
        <span v-else class="dl-tag dl-tag--signal ver-installing">
          {{ `安装中` }}
        </span>
      </div>
    </div>
  </section>
</template>

<style lang="scss" scoped>
.dl-section-label {
  .ver-label-spacer {
    flex: 1;
  }
}

.ver-channels {
  margin-right: 4px;
}

.ver-search {
  width: 190px;
  height: 28px;
  margin-right: 8px;
}

.ver-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;

  & + .ver-row {
    border-top: 1px solid var(--dl-line);
  }
}

.ver-mark {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  flex-shrink: 0;
  border-radius: var(--dl-r-ctl);
  background: var(--dl-run-soft);
  color: var(--dl-run);

  &.is-idle {
    background: var(--dl-surface-3);
    color: var(--dl-ink-3);
  }
}

.ver-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.ver-title {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ver-name {
  font-size: var(--dl-text-lg);
  font-weight: 600;
  color: var(--dl-ink);
}

.ver-path,
.ver-sub {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ver-used {
  flex-shrink: 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.ver-install {
  flex-shrink: 0;
}

.ver-installing {
  flex-shrink: 0;
}

.ver-progress {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 6px;
  max-width: 460px;

  :deep(.arco-progress) {
    flex: 1;
    margin: 0;
  }
}

.ver-install-state {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: 6px;
  max-width: 560px;
}

.ver-failed {
  margin: 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-danger);
}

.ver-cancelled {
  margin: 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

/* The install's own output. It is a console, so it stays a console: its tail,
   monospaced and quiet, scrolled rather than truncated — the failure reason
   above it is written from these same lines. */
.ver-log {
  display: flex;
  flex-direction: column;
  gap: 1px;
  max-height: 132px;
  overflow: auto;
  padding: 8px 10px;
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface-3);
}

.ver-log-line {
  font-size: 11px;
  line-height: 1.5;
  color: var(--dl-ink-3);
  white-space: pre-wrap;
  word-break: break-all;
}
</style>

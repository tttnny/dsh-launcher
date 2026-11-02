<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { useAction } from '@/composables/useAction'
import { useCopy } from '@/composables/useCopy'
import { useProfiles } from '@/composables/useProfiles'
import { useLauncherStore } from '@/stores/launcher'
import type { DshHome } from '@/api/types'
import PageHeader from '@/components/PageHeader.vue'
import EmptyState from '@/components/EmptyState.vue'
import DlIcon from '@/components/DlIcon.vue'
import HomeFormDialog from '@/components/HomeFormDialog.vue'

// 数据与配置：左边管 DSH_HOME（数据目录），右边管选中目录里的 Profile。
// DSH_HOME 是启动器保存的一条记录，所以机器连不上时记录仍在；Profile 必须
// 从目录里读，读不到就说读不到，不能把「没读到」显示成「没有」。

const store = useLauncherStore()
const { copy } = useCopy()
const {
  profilesError,
  loadForHome,
  invalidateHome,
  forgetSelection,
  forgetHome,
} = useProfiles()

const hostLabel = computed(() => {
  const host = store.currentHost
  return host?.builtin ? '本机' : (host?.display_name ?? store.currentHostId)
})
const remote = computed(() => store.currentHost?.builtin === false)
const hostUnreachable = computed(
  () => store.currentHost?.status?.state === 'unreachable',
)

const homes = computed(() => store.homes)
const homesLoading = computed(
  () =>
    !store.loaded ||
    (!store.homeError && store.homesByHost[store.currentHostId] === undefined),
)

const selectedHomeId = ref<string | undefined>(undefined)
const selectedHome = computed(
  () => homes.value.find((h) => h.id === selectedHomeId.value) ?? null,
)
const homeInstances = computed(() =>
  selectedHome.value ? store.instancesOfHome(selectedHome.value.id) : [],
)

function usedBy(homeId: string): number {
  return store.instancesOfHome(homeId).length
}

watch(
  [() => store.currentHostId, homes],
  () => {
    const list = homes.value
    if (
      !selectedHomeId.value ||
      !list.some((h) => h.id === selectedHomeId.value)
    ) {
      selectedHomeId.value = list[0]?.id
    }
  },
  { immediate: true },
)

// --- 数据目录 ----------------------------------------------------------------

const homeDialogVisible = ref(false)

function openCreateHome() {
  homeDialogVisible.value = true
}

function onHomeCreated(id: string) {
  selectedHomeId.value = id
}

function onSelectHost(id: string) {
  if (id === store.currentHostId) return
  void store.selectHost(id)
}

const dirAction = useAction((id: string) => api.openHomeDirectory(id), {
  key: (id) => id,
  success: (path) => '已打开目录：' + path,
})

const removeAction = useAction(
  async (id: string, name: string) => {
    await api.removeHome(id, store.currentHostId)
    forgetHome(id)
    await store.refreshHomes(store.currentHostId)
    return name
  },
  {
    key: (id) => id,
    success: (name) => '已移除数据目录「' + name + '」',
  },
)

function confirmRemoveHome(home: DshHome) {
  Modal.confirm({
    title: '移除数据目录「' + home.name + '」？',
    content:
      '只删除启动器里的这条记录，磁盘上「' +
      home.path +
      '」里的文件和会话都不会动。',
    okText: '移除',
    cancelText: '取消',
    okButtonProps: { status: 'danger' },
    onOk: () => removeAction.run(home.id, home.name),
  })
}

// --- Profile -----------------------------------------------------------------

const profiles = ref<string[]>([])
const profilesLoading = ref(false)
const profilesErrorMsg = ref('')
const adding = ref(false)
const newName = ref('')
const renaming = ref<string | null>(null)
const renameValue = ref('')
const copying = ref<string | null>(null)
const copyValue = ref('')

function resetEditors() {
  adding.value = false
  newName.value = ''
  renaming.value = null
  renameValue.value = ''
  copying.value = null
  copyValue.value = ''
}

async function loadProfiles() {
  const homeId = selectedHomeId.value
  profiles.value = []
  profilesErrorMsg.value = ''
  if (!homeId) {
    profilesLoading.value = false
    return
  }
  profilesLoading.value = true
  const list = await loadForHome(homeId)
  if (selectedHomeId.value !== homeId) return
  if (list === undefined) {
    profilesErrorMsg.value = profilesError[homeId] || '读取失败'
  } else {
    profiles.value = list
  }
  profilesLoading.value = false
}

watch(
  selectedHomeId,
  () => {
    resetEditors()
    void loadProfiles()
  },
  { immediate: true },
)

function retryProfiles() {
  const homeId = selectedHomeId.value
  if (!homeId) return
  invalidateHome(homeId)
  void loadProfiles()
}

function startAdd() {
  renaming.value = null
  copying.value = null
  adding.value = true
  newName.value = ''
}

function cancelAdd() {
  adding.value = false
  newName.value = ''
}

const createAction = useAction(
  (homeId: string, name: string) => api.createProfile(homeId, name),
  { success: (name) => 'Profile「' + name + '」已创建' },
)

async function submitAdd() {
  const homeId = selectedHomeId.value
  const name = newName.value.trim()
  if (!homeId || !name) return
  const created = await createAction.run(homeId, name)
  if (created === undefined) return
  invalidateHome(homeId)
  adding.value = false
  newName.value = ''
  await loadProfiles()
}

function startRename(name: string) {
  adding.value = false
  copying.value = null
  renaming.value = name
  renameValue.value = name
}

function cancelRename() {
  renaming.value = null
}

const renameAction = useAction(
  (homeId: string, oldName: string, next: string) =>
    api.renameProfile(homeId, oldName, next),
  {
    key: (_homeId, oldName) => oldName,
    success: () => 'Profile 已重命名',
  },
)

async function submitRename() {
  const homeId = selectedHomeId.value
  const oldName = renaming.value
  const next = renameValue.value.trim()
  if (!homeId || !oldName || !next || oldName === next) {
    renaming.value = null
    return
  }
  const renamed = await renameAction.run(homeId, oldName, next)
  if (renamed === undefined) return
  invalidateHome(homeId)
  forgetSelection(
    oldName,
    store.instancesOfHome(homeId).map((i) => i.id),
  )
  await store.refreshInstances()
  renaming.value = null
  await loadProfiles()
}

function startCopy(name: string) {
  adding.value = false
  renaming.value = null
  copying.value = name
  copyValue.value = name + '-copy'
}

function cancelCopy() {
  copying.value = null
}

const copyAction = useAction(
  (homeId: string, source: string, name: string) =>
    api.copyProfile(homeId, source, name),
  { success: (name) => '已复制为「' + name + '」' },
)

async function submitCopy() {
  const homeId = selectedHomeId.value
  const source = copying.value
  const name = copyValue.value.trim()
  if (!homeId || !source || !name) {
    copying.value = null
    return
  }
  const copied = await copyAction.run(homeId, source, name)
  if (copied === undefined) return
  invalidateHome(homeId)
  copying.value = null
  await loadProfiles()
}

const deleteAction = useAction(
  async (homeId: string, name: string) => {
    await api.deleteProfile(homeId, name)
    return name
  },
  {
    key: (_homeId, name) => name,
    success: (name) => 'Profile「' + name + '」已删除',
  },
)

function defaultUsersOf(name: string) {
  return homeInstances.value.filter((i) => i.default_profile === name)
}

function lastUsersOf(name: string) {
  return homeInstances.value.filter(
    (i) => i.default_profile !== name && i.last_profile === name,
  )
}

function deleteImpact(name: string): string {
  const defaults = defaultUsersOf(name).map((i) => i.name)
  const lasts = lastUsersOf(name).map((i) => i.name)
  const parts: string[] = []
  if (defaults.length) {
    parts.push('实例「' + defaults.join('」「') + '」默认使用它')
  }
  if (lasts.length) {
    parts.push('实例「' + lasts.join('」「') + '」上次使用它')
  }
  return parts.length
    ? '删除后，' + parts.join('，') + '，这些实例需要重新选择 Profile。'
    : '当前没有实例使用这个 Profile。'
}

function onDeleteProfile(name: string) {
  const homeId = selectedHomeId.value
  if (!homeId) return
  Modal.confirm({
    title: '删除 Profile「' + name + '」？',
    content:
      '从数据目录「' +
      (selectedHome.value?.name ?? '') +
      '」删除这个 Profile。' +
      deleteImpact(name) +
      '会话数据仍保留在数据目录里，删除后不可恢复。',
    okText: '删除',
    cancelText: '取消',
    okButtonProps: { status: 'danger' },
    onOk: async () => {
      const deleted = await deleteAction.run(homeId, name)
      if (deleted === undefined) return
      invalidateHome(homeId)
      forgetSelection(
        name,
        store.instancesOfHome(homeId).map((i) => i.id),
      )
      await store.refreshInstances()
      await loadProfiles()
    },
  })
}

function busyProfile(name: string): boolean {
  return (
    !!renameAction.busy[name] ||
    !!copyAction.busy[name] ||
    !!deleteAction.busy[name]
  )
}
</script>

<template>
  <div class="page">
    <PageHeader
      title="数据与配置"
      description="DSH_HOME 保存会话、凭据和配置；Profile 是其中一套插件组合。"
    >
      <template #actions>
        <button class="btn btn--primary" @click="openCreateHome">
          <DlIcon name="plus" :size="14" />
          新建数据目录
        </button>
      </template>
    </PageHeader>

    <div class="machine-bar">
      <button
        v-for="h in store.hosts"
        :key="h.id"
        class="chip"
        :class="{ 'is-active': h.id === store.currentHostId }"
        @click="onSelectHost(h.id)"
      >
        {{ h.display_name }}
      </button>
    </div>

    <div
      v-if="hostUnreachable && !store.homeError"
      class="notice notice--warn host-note"
    >
      <div class="notice-body">
        这台机器现在连不上。数据目录是启动器保存的记录，仍会显示；Profile
        列表要等它恢复后才能读取。
      </div>
    </div>

    <div v-if="store.homeError" class="notice notice--danger">
      <div class="notice-body">
        <div class="notice-title">读不到这台机器的数据目录</div>
        <div>{{ store.homeError }}</div>
        <div class="notice-retry">
          <button
            class="btn btn--quiet btn--sm"
            @click="store.refreshHomes(store.currentHostId)"
          >
            重试
          </button>
        </div>
      </div>
    </div>

    <div v-else-if="homesLoading" class="surface">
      <p class="loading-row">加载中…</p>
    </div>

    <div v-else-if="store.homes.length === 0" class="surface">
      <EmptyState
        icon="data"
        title="还没有数据目录"
        description="DSH_HOME 保存会话、凭据和配置。新建一个，再在里面创建 Profile。"
      >
        <button class="btn btn--primary" @click="openCreateHome">
          新建数据目录
        </button>
      </EmptyState>
    </div>

    <div v-else class="data-split">
      <aside class="dir-pane">
        <div class="pane-head">
          <span class="pane-title">数据目录</span>
        </div>

        <div class="dir-list">
          <div
            v-for="h in store.homes"
            :key="h.id"
            class="dir-item"
            :class="{ 'is-selected': h.id === selectedHomeId }"
            role="button"
            tabindex="0"
            @click="selectedHomeId = h.id"
            @keydown.enter="selectedHomeId = h.id"
          >
            <div class="dir-main">
              <div class="dir-name-row">
                <span class="dir-name">{{ h.name }}</span>
                <span v-if="usedBy(h.id) > 0" class="tag tag--muted"
                  >使用中</span
                >
              </div>
              <span class="dir-path mono" :title="h.path">{{ h.path }}</span>
            </div>
            <a-dropdown trigger="click" position="br" :hide-on-select="true">
              <button class="icon-btn" title="更多" @click.stop>
                <DlIcon name="more" :size="15" />
              </button>
              <template #content>
                <a-doption v-if="remote" @click="copy(h.path, '已复制路径')">
                  <DlIcon name="copy" :size="13" />
                  复制路径
                </a-doption>
                <a-doption v-else @click="dirAction.run(h.id)">
                  <DlIcon name="folder" :size="13" />
                  打开目录
                </a-doption>
                <a-doption
                  :disabled="usedBy(h.id) > 0"
                  @click="usedBy(h.id) === 0 && confirmRemoveHome(h)"
                >
                  <DlIcon name="trash" :size="13" />
                  移除
                </a-doption>
              </template>
            </a-dropdown>
          </div>
        </div>
      </aside>

      <section class="profile-pane">
        <div class="pane-head">
          <span class="pane-title">配置</span>
          <span v-if="selectedHome" class="pane-sub">{{
            selectedHome.name
          }}</span>
          <span class="spacer" />
          <button
            v-if="!adding"
            class="btn btn--quiet btn--sm"
            @click="startAdd"
          >
            <DlIcon name="plus" :size="13" />
            新建配置
          </button>
        </div>

        <div v-if="adding" class="list-row profile-edit">
          <input
            v-model="newName"
            class="input"
            placeholder="输入配置名称"
            @keydown.enter="submitAdd"
          />
          <div class="row-actions">
            <button
              class="btn btn--primary btn--sm"
              :disabled="!newName.trim() || !!createAction.busy['*']"
              @click="submitAdd"
            >
              创建
            </button>
            <button class="btn btn--quiet btn--sm" @click="cancelAdd">
              取消
            </button>
          </div>
        </div>

        <p v-if="profilesLoading" class="loading-row">加载中…</p>

        <div v-else-if="profilesErrorMsg" class="loading-row error-text">
          <span>{{ profilesErrorMsg }}</span>
          <button class="btn btn--quiet btn--sm" @click="retryProfiles">
            重试
          </button>
        </div>

        <EmptyState
          v-else-if="profiles.length === 0 && !adding"
          icon="data"
          title="还没有配置"
        >
          <button class="btn btn--primary" @click="startAdd">新建配置</button>
        </EmptyState>

        <template v-for="p in profiles" :key="p">
          <div v-if="renaming === p" class="list-row profile-edit">
            <input
              v-model="renameValue"
              class="input"
              @keydown.enter="submitRename"
            />
            <div class="row-actions">
              <button
                class="btn btn--primary btn--sm"
                :disabled="!!renameAction.busy[p]"
                @click="submitRename"
              >
                保存
              </button>
              <button class="btn btn--quiet btn--sm" @click="cancelRename">
                取消
              </button>
            </div>
          </div>

          <div v-else-if="copying === p" class="list-row profile-edit">
            <input
              v-model="copyValue"
              class="input"
              @keydown.enter="submitCopy"
            />
            <div class="row-actions">
              <button
                class="btn btn--primary btn--sm"
                :disabled="!!copyAction.busy['*']"
                @click="submitCopy"
              >
                保存
              </button>
              <button class="btn btn--quiet btn--sm" @click="cancelCopy">
                取消
              </button>
            </div>
          </div>

          <div v-else class="list-row profile-row">
            <div class="row-main">
              <div class="row-title">
                <span class="mono profile-name">{{ p }}</span>
              </div>
              <div
                v-if="defaultUsersOf(p).length > 0 || lastUsersOf(p).length > 0"
                class="row-meta"
              >
                <span
                  v-for="inst in [...defaultUsersOf(p), ...lastUsersOf(p)]"
                  :key="inst.id"
                  class="tag tag--muted"
                  >{{ inst.name }}</span
                >
              </div>
            </div>
            <a-dropdown trigger="click" position="br" :hide-on-select="true">
              <button class="icon-btn" title="更多">
                <DlIcon name="more" :size="15" />
              </button>
              <template #content>
                <a-doption :disabled="busyProfile(p)" @click="startRename(p)">
                  <DlIcon name="settings" :size="13" />
                  重命名
                </a-doption>
                <a-doption :disabled="busyProfile(p)" @click="startCopy(p)">
                  <DlIcon name="copy" :size="13" />
                  复制
                </a-doption>
                <a-doption
                  :disabled="busyProfile(p)"
                  @click="onDeleteProfile(p)"
                >
                  <DlIcon name="trash" :size="13" />
                  删除
                </a-doption>
              </template>
            </a-dropdown>
          </div>
        </template>
      </section>
    </div>

    <HomeFormDialog
      v-model:visible="homeDialogVisible"
      :host-id="store.currentHostId"
      :host-label="hostLabel"
      :remote="remote"
      @created="onHomeCreated"
    />
  </div>
</template>

<style lang="scss" scoped>
.machine-bar {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 2px;
  margin-bottom: 16px;
  padding: 3px;
  border-radius: 8px;
  background: var(--dl-surface-3);
}

.host-note {
  margin-bottom: 16px;
}

.notice-retry {
  margin-top: 8px;
}

.loading-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0;
  padding: 22px 16px;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-md);
}

.data-split {
  display: grid;
  grid-template-columns: 300px minmax(0, 1fr);
  min-height: 380px;
  overflow: hidden;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-card);
  background: var(--dl-surface);
}

.dir-pane {
  display: flex;
  flex-direction: column;
  min-width: 0;
  border-right: 1px solid var(--dl-line);
  background: var(--dl-surface-2);
}

.profile-pane {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.pane-head {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 44px;
  padding: 8px 12px 8px 14px;
  border-bottom: 1px solid var(--dl-line);
}

.pane-title {
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
  font-weight: 600;
}

.pane-sub {
  min-width: 0;
  overflow: hidden;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dir-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-height: 520px;
  padding: 6px;
  overflow-y: auto;
}

.dir-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 8px 8px 10px;
  border-radius: 7px;
  cursor: pointer;

  &:hover {
    background: var(--dl-rail-hover);
  }

  &.is-selected {
    background: var(--dl-accent-soft);

    .dir-name {
      color: var(--dl-accent-ink);
    }
  }
}

.dir-main {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.dir-name-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.dir-name {
  overflow: hidden;
  color: var(--dl-ink);
  font-size: var(--dl-text-md);
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dir-path {
  display: block;
  max-width: 100%;
  overflow: hidden;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.profile-pane .list-row {
  padding-right: 12px;
  padding-left: 16px;
}

.profile-pane :deep(.empty) {
  padding: 42px 20px;
}

@media (max-width: 900px) {
  .data-split {
    grid-template-columns: 1fr;
    min-height: 0;
  }

  .dir-pane {
    border-right: none;
    border-bottom: 1px solid var(--dl-line);
  }

  .dir-list {
    max-height: 220px;
  }
}

.profile-edit {
  gap: 10px;

  .input {
    flex: 1;
    max-width: 320px;
  }
}

.profile-name {
  font-weight: 600;
}
</style>

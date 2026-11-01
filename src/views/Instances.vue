<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { DshInstance } from '@/api/types'
import { useLauncherStore } from '@/stores/launcher'
import { useProfiles } from '@/composables/useProfiles'
import DlIcon from '@/components/DlIcon.vue'
import PageHeader from '@/components/PageHeader.vue'
import EmptyState from '@/components/EmptyState.vue'
import InstanceRow from '@/components/InstanceRow.vue'
import NewInstanceDialog from '@/components/NewInstanceDialog.vue'
import ProfilePickerDialog from '@/components/ProfilePickerDialog.vue'

// 实例页是启动器的主页：所有机器上的实例都在这里，按状态分组，
// 每一行都有一个明确的下一步动作。

const store = useLauncherStore()
const router = useRouter()
const route = useRoute()
const { loadForHome, selectionByInstance, profilesError } = useProfiles()

const search = ref('')
const hostFilter = ref<string>(store.allHosts ? 'all' : store.currentHostId)

const newDialogVisible = ref(false)
const newPreset = ref<{
  name: string
  versionId: string
  homeId: string
  hostId?: string
} | null>(null)

const pickerVisible = ref(false)
const pickerInstance = ref<DshInstance | null>(null)
const pickerProfiles = ref<string[]>([])

const focusId = computed(() =>
  typeof route.query.focus === 'string' ? route.query.focus : '',
)

const hosts = computed(() => store.hosts)
const showMachineFilter = computed(() => hosts.value.length > 1)

const filtered = computed(() => {
  const q = search.value.trim().toLowerCase()
  return store.instances
    .filter((inst) => {
      if (
        hostFilter.value !== 'all' &&
        (inst.host_id ?? 'host-local') !== hostFilter.value
      )
        return false
      if (!q) return true
      return (
        inst.name.toLowerCase().includes(q) ||
        inst.id.toLowerCase().includes(q) ||
        (store.versionById(inst.version_id)?.version ?? '')
          .toLowerCase()
          .includes(q)
      )
    })
    .sort((a, b) => a.name.localeCompare(b.name, 'zh-Hans-CN'))
})

const groups = computed(() => {
  const running: DshInstance[] = []
  const stopped: DshInstance[] = []
  const unreachable: DshInstance[] = []
  for (const inst of filtered.value) {
    const state = store.statusOf(inst.id).state
    if (state === 'running' || state === 'starting') running.push(inst)
    else if (state === 'unreachable') unreachable.push(inst)
    else stopped.push(inst)
  }
  return [
    { key: 'running', label: '运行中', items: running },
    { key: 'stopped', label: '已停止', items: stopped },
    { key: 'unreachable', label: '主机不可达', items: unreachable },
  ].filter((g) => g.items.length > 0)
})

const envOk = computed(
  () => !!store.runtime?.node?.installed && !!store.runtime?.pnpm?.installed,
)

const showMachine = computed(
  () => hostFilter.value === 'all' && hosts.value.length > 1,
)

function pickHost(id: string) {
  hostFilter.value = id
  store.setAllHosts(id === 'all')
  if (id !== 'all') void store.selectHost(id)
}

function openNew() {
  newPreset.value = null
  newDialogVisible.value = true
}

function duplicate(inst: DshInstance) {
  newPreset.value = {
    name: inst.name,
    versionId: inst.version_id,
    homeId: inst.home_id,
    hostId: inst.host_id,
  }
  newDialogVisible.value = true
}

function goInstall() {
  void router.push('/install')
}

function goLocalMachine() {
  void store.selectHost('host-local')
  void router.push('/machines/host-local')
}

async function startInstance(inst: DshInstance) {
  const list = await loadForHome(inst.home_id)
  if (!list) {
    Message.error(
      `读取配置失败：${profilesError[inst.home_id] ?? '请稍后重试'}`,
    )
    return
  }
  let profile = selectionByInstance[inst.id]
  if (!profile || !list.includes(profile)) {
    profile =
      (inst.last_profile &&
        list.includes(inst.last_profile) &&
        inst.last_profile) ||
      (inst.default_profile &&
        list.includes(inst.default_profile) &&
        inst.default_profile) ||
      undefined
  }
  if (!profile && list.length === 1) profile = list[0]
  if (!profile && list.length > 1) {
    pickerInstance.value = inst
    pickerProfiles.value = list
    pickerVisible.value = true
    return
  }
  if (!profile) {
    Message.warning(
      '这个数据目录里没有可用的 Profile，请先在「数据」页创建一个',
    )
    return
  }
  await startWithProfile(inst, profile)
}

async function startWithProfile(inst: DshInstance, profile: string) {
  try {
    await api.startInstance(inst.id, profile)
    selectionByInstance[inst.id] = profile
    if (store.settings.last_instance_id !== inst.id) {
      const settings = await api.updateSettings({ last_instance_id: inst.id })
      store.settings = settings
    }
    Message.success(`正在启动「${inst.name}」`)
  } catch (e) {
    Message.error(String(e))
  }
}

async function onPickProfile(profile: string) {
  const inst = pickerInstance.value
  if (!inst) return
  await startWithProfile(inst, profile)
  pickerInstance.value = null
}

async function onCreated(id: string) {
  await store.refreshInstances()
  void router.push({ name: 'instance-detail', params: { id } })
}
</script>

<template>
  <div class="page">
    <PageHeader
      title="实例"
      description="启动、打开和停止这台 Mac 与其他机器上的 DSH 实例。"
    >
      <template #actions>
        <button class="btn btn--quiet" @click="goInstall">
          <DlIcon name="download" :size="14" />
          安装版本
        </button>
        <button class="btn btn--primary" @click="openNew">
          <DlIcon name="plus" :size="14" />
          新建实例
        </button>
      </template>
    </PageHeader>

    <div
      v-if="!envOk && store.instances.length > 0"
      class="notice notice--warn env-notice"
    >
      <span class="notice-title">本机环境未就绪</span>
      <button class="btn btn--quiet btn--sm" @click="goLocalMachine">
        检查环境
      </button>
    </div>

    <div v-if="store.instances.length === 0" class="surface">
      <div v-if="!envOk" class="startup-note">
        <span class="notice-title">本机环境未就绪</span>
        <button class="btn btn--quiet btn--sm" @click="goLocalMachine">
          检查环境
        </button>
      </div>
      <EmptyState
        icon="instance"
        title="还没有 DSH 实例"
        description="安装一个 DSH 版本，安装完成后会自动创建第一个实例。"
      >
        <button class="btn btn--primary" @click="goInstall">
          <DlIcon name="download" :size="14" />
          安装第一个 DSH 版本
        </button>
        <button class="btn btn--quiet" @click="openNew">新建实例</button>
      </EmptyState>
    </div>

    <template v-else>
      <div class="board-toolbar">
        <div v-if="showMachineFilter" class="host-chips">
          <button
            class="chip"
            :class="{ 'is-active': hostFilter === 'all' }"
            @click="pickHost('all')"
          >
            全部机器
          </button>
          <button
            v-for="h in hosts"
            :key="h.id"
            class="chip"
            :class="{ 'is-active': hostFilter === h.id }"
            @click="pickHost(h.id)"
          >
            {{ h.display_name }}
          </button>
        </div>
        <div class="spacer" />
        <label class="search-box">
          <DlIcon name="search" :size="14" />
          <input v-model="search" class="search-input" placeholder="搜索实例" />
          <button
            v-if="search"
            class="icon-btn search-clear"
            @click="search = ''"
          >
            <DlIcon name="close" :size="12" />
          </button>
        </label>
      </div>

      <div v-if="filtered.length === 0" class="surface">
        <EmptyState
          icon="search"
          title="没有匹配的实例"
          description="换个关键词，或者切换到「全部机器」。"
        />
      </div>

      <template v-else>
        <section
          v-for="group in groups"
          :key="group.key"
          class="instance-group"
        >
          <div class="group-head">
            <span class="group-dot" :class="'is-' + group.key" />
            <span class="group-title">{{ group.label }}</span>
            <span class="group-count">{{ group.items.length }}</span>
          </div>
          <div class="surface">
            <InstanceRow
              v-for="inst in group.items"
              :key="inst.id"
              :inst="inst"
              :show-machine="showMachine"
              :focus="inst.id === focusId"
              @start="startInstance"
              @duplicate="duplicate"
              @changed="() => store.refreshInstances()"
            />
          </div>
        </section>
      </template>
    </template>

    <NewInstanceDialog
      v-model:visible="newDialogVisible"
      :preset="newPreset"
      @created="onCreated"
    />
    <ProfilePickerDialog
      v-model:visible="pickerVisible"
      :instance="pickerInstance"
      :profiles="pickerProfiles"
      @pick="onPickProfile"
    />
  </div>
</template>

<style lang="scss" scoped>
.env-notice {
  margin-bottom: 14px;
  align-items: center;
}

.startup-note {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 12px 16px;
  border-bottom: 1px solid var(--dl-line);
  background: var(--dl-warn-soft);
  color: var(--dl-warn);
}

.startup-note-text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: var(--dl-text-sm);
}

.board-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 10px;
}

.host-chips {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 3px;
  border-radius: 8px;
  background: var(--dl-surface-3);
}

.search-box {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 280px;
  height: 32px;
  padding: 0 9px;
  border: 1px solid var(--dl-line-strong);
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface);
  color: var(--dl-ink-3);

  &:focus-within {
    border-color: var(--dl-accent-line);
    box-shadow: 0 0 0 3px var(--dl-accent-soft);
  }
}

.search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--dl-ink);
  font-family: inherit;
  font-size: var(--dl-text-md);

  &::placeholder {
    color: var(--dl-ink-4);
  }
}

.search-clear {
  width: 20px;
  height: 20px;
}

.board-summary {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 2px 20px;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.summary-dot {
  width: 3px;
  height: 3px;
  border-radius: 50%;
  background: var(--dl-ink-4);
}

.instance-group {
  margin-top: 24px;

  &:first-of-type {
    margin-top: 0;
  }
}

.group-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 2px 8px;
}

.group-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--dl-ink-4);

  &.is-running {
    background: var(--dl-run);
  }

  &.is-unreachable {
    background: var(--dl-warn);
  }
}

.group-title {
  font-size: var(--dl-text-sm);
  font-weight: 650;
  color: var(--dl-ink-2);
}

.group-count {
  color: var(--dl-ink-4);
  font-family: var(--dl-font-mono);
  font-size: var(--dl-text-xs);
}
</style>

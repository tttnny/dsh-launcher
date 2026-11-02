<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import type { DshInstance } from '@/api/types'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import { useCopy } from '@/composables/useCopy'
import { useProfiles } from '@/composables/useProfiles'
import DlIcon from '@/components/DlIcon.vue'
import InstanceAvatar from '@/components/InstanceAvatar.vue'

// 实例列表里的一行：状态、身份、和每天要用的动作。默认配置和端口这类设置
// 收在详情页，不占列表的空间。

const props = defineProps<{
  inst: DshInstance
  showMachine: boolean
  focus?: boolean
}>()

const emit = defineEmits<{
  start: [inst: DshInstance]
  changed: []
  duplicate: [inst: DshInstance]
}>()

const router = useRouter()
const store = useLauncherStore()
const { copy } = useCopy()
const { forgetInstance } = useProfiles()
const highlighted = ref(props.focus ?? false)

watch(
  () => props.focus,
  (v) => {
    if (v) highlighted.value = true
  },
)

const status = computed(() => store.statusOf(props.inst.id))
const running = computed(() => status.value.state === 'running')
const starting = computed(() => status.value.state === 'starting')
const unreachable = computed(() => status.value.state === 'unreachable')

const versionName = computed(
  () => store.versionById(props.inst.version_id)?.version ?? '未知版本',
)
const homeName = computed(
  () => store.homeById(props.inst.home_id)?.name ?? '未知数据目录',
)
const homePath = computed(() => store.homeById(props.inst.home_id)?.path ?? '')
const hostName = computed(
  () => store.hostById(props.inst.host_id)?.display_name ?? '本机',
)

const openAction = useAction((id: string) => api.openInstanceWindow(id), {
  key: (id) => id,
})
const stopAction = useAction((id: string) => api.stopInstance(id), {
  key: (id) => id,
  success: () => '实例已停止',
})
const restartAction = useAction((id: string) => api.restartInstance(id), {
  key: (id) => id,
  success: () => '实例已重启',
})
const retryAction = useAction(
  async (id: string) => {
    await api.refreshInstanceStatus(id)
    store.hosts = await api.listHosts()
    return true
  },
  { key: (id) => id },
)
const terminalAction = useAction(
  () => api.openInstanceTerminal(props.inst.id),
  {
    success: (label) => `已打开终端：${label}`,
  },
)
const logAction = useAction(() => api.openInstanceLog(props.inst.id), {
  success: (path) => `已在访达中定位日志：${path}`,
})
const dirAction = useAction(() => api.openInstanceDirectory(props.inst.id), {
  success: () => '已在访达中打开数据目录',
})

const busy = computed(
  () =>
    !!openAction.busy[props.inst.id] ||
    !!stopAction.busy[props.inst.id] ||
    !!restartAction.busy[props.inst.id],
)

function touchLastUsed() {
  if (store.settings.last_instance_id === props.inst.id) return
  void api.updateSettings({ last_instance_id: props.inst.id }).then((s) => {
    store.settings = s
  })
}

async function onOpen() {
  const ok = await openAction.run(props.inst.id)
  if (ok === undefined) return
  touchLastUsed()
}

async function onStop() {
  await stopAction.run(props.inst.id)
}

async function onRestart() {
  await restartAction.run(props.inst.id)
}

async function onRetry() {
  await retryAction.run(props.inst.id)
}

function openDetail() {
  void router.push({ name: 'instance-detail', params: { id: props.inst.id } })
}

function onRowClick(event: MouseEvent) {
  if (
    (event.target as HTMLElement | null)?.closest(
      'button, a, input, .arco-dropdown',
    )
  )
    return
  openDetail()
}

async function copyAddress() {
  if (status.value.url) await copy(status.value.url, '地址已复制')
}

const deleteAction = useAction(async () => {
  await api.deleteInstance(props.inst.id, props.inst.host_id)
  forgetInstance(props.inst.id)
  await store.refreshInstances()
  return true
})

function confirmDelete() {
  Modal.confirm({
    title: `删除实例「${props.inst.name}」？`,
    content: `只删除启动器里的这条实例记录：DSH 版本和 DSH_HOME「${homeName.value}」都会保留，磁盘上的数据不动。`,
    okText: '删除',
    cancelText: '取消',
    okButtonProps: { status: 'danger' },
    onOk: async () => {
      const ok = await deleteAction.run()
      if (ok !== true) return
      Message.success(`已删除实例「${props.inst.name}」`)
      emit('changed')
    },
  })
}
</script>

<template>
  <div
    class="instance-row"
    :class="{ 'is-live': running, 'is-focus': highlighted }"
    @click="onRowClick"
  >
    <InstanceAvatar :id="inst.id" :icon="inst.icon" :size="32" />

    <div class="row-main">
      <div class="row-title">
        <span class="state-dot" :class="'is-' + status.state" />
        <span class="inst-name" :title="inst.name">{{ inst.name }}</span>
      </div>
      <div class="row-meta">
        <template v-if="showMachine">
          <span class="meta-item">{{ hostName }}</span>
          <span class="meta-dot" />
        </template>
        <span class="meta-item mono" :title="inst.version_id">{{
          versionName
        }}</span>
      </div>
    </div>

    <div class="row-actions" @click.stop>
      <template v-if="running">
        <button class="btn btn--run" :disabled="busy" @click="onOpen">
          打开
        </button>
      </template>
      <template v-else-if="starting">
        <button class="btn btn--quiet" disabled>
          <span class="spinner" />
          启动中
        </button>
      </template>
      <template v-else-if="unreachable">
        <button
          class="btn btn--quiet"
          :disabled="!!retryAction.busy[inst.id]"
          @click="onRetry"
        >
          重试
        </button>
      </template>
      <template v-else>
        <button class="btn btn--primary" @click="emit('start', inst)">
          启动
        </button>
      </template>

      <a-dropdown trigger="click" position="br" :hide-on-select="true">
        <button class="icon-btn" title="更多操作">
          <DlIcon name="more" :size="16" />
        </button>
        <template #content>
          <a-doption v-if="running || starting" @click="onStop">
            <DlIcon name="stop" :size="13" />
            停止
          </a-doption>
          <a-doption v-if="running" @click="onRestart">
            <DlIcon name="restart" :size="13" />
            重启
          </a-doption>
          <a-doption @click="terminalAction.run()">
            <DlIcon name="terminal" :size="13" />
            打开终端
          </a-doption>
          <a-doption @click="logAction.run()">
            <DlIcon name="log" :size="13" />
            查看日志
          </a-doption>
          <a-doption v-if="status.url" @click="copyAddress">
            <DlIcon name="copy" :size="13" />
            复制地址
          </a-doption>
          <a-doption
            v-if="!store.hostById(inst.host_id)?.builtin"
            @click="copy(homePath, '数据目录路径已复制')"
          >
            <DlIcon name="copy" :size="13" />
            复制数据目录
          </a-doption>
          <a-doption v-else @click="dirAction.run()">
            <DlIcon name="folder" :size="13" />
            打开数据目录
          </a-doption>
          <a-doption @click="openDetail">
            <DlIcon name="settings" :size="13" />
            实例设置
          </a-doption>
          <a-doption @click="emit('duplicate', inst)">
            <DlIcon name="copy" :size="13" />
            复制实例
          </a-doption>
          <a-doption @click="confirmDelete">
            <span class="menu-danger"
              ><DlIcon name="trash" :size="13" /> 删除实例</span
            >
          </a-doption>
        </template>
      </a-dropdown>
    </div>
  </div>
</template>
<style lang="scss" scoped>
.instance-row {
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 58px;
  padding: 10px 14px;
  background: var(--dl-surface);
  cursor: pointer;
  transition: background-color var(--dl-fast) var(--dl-ease);

  & + & {
    border-top: 1px solid var(--dl-line);
  }

  &:hover {
    background: var(--dl-surface-2);
  }

  &.is-live {
    background: var(--dl-live-wash);
  }

  &.is-live:hover {
    background: var(--dl-surface-2);
  }

  &.is-focus {
    animation: focus-flash 1.6s var(--dl-ease);
  }
}

@keyframes focus-flash {
  from {
    background-color: var(--dl-accent-soft);
  }
}

.state-dot {
  width: 6px;
  height: 6px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--dl-ink-4);

  &.is-running {
    background: var(--dl-run);
  }

  &.is-starting {
    background: var(--dl-warn);
  }

  &.is-exited {
    background: var(--dl-danger);
  }

  &.is-unreachable {
    background: var(--dl-warn);
  }
}

.inst-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.meta-item {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.meta-dot {
  width: 3px;
  height: 3px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--dl-ink-4);
}

.row-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.menu-danger {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--dl-danger);
}
</style>

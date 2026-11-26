<script setup lang="ts">
import { computed, ref } from 'vue'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { FullDiskAccessStatus } from '@/api/types'
import launcherIcon from '../../src-tauri/icons/128x128@2x.png'

const props = defineProps<{
  visible: boolean
  status: FullDiskAccessStatus | null
  loading: boolean
  error: string
  settingsOpened: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'refresh'): void
}>()

const dragging = ref(false)
const revealing = ref(false)

const granted = computed(() => !!props.status?.granted)
const appPath = computed(() => props.status?.app_path ?? '')
const checking = computed(() => props.loading && !props.status && !props.error)
const problem = computed(() => props.error || props.status?.error || '')
const leadText = computed(() =>
  props.settingsOpened
    ? '已打开系统设置。把拖动面板里的应用图标拖进列表，再打开开关，状态会自动更新。'
    : '请先在系统设置里打开「完全磁盘访问」，再把拖动面板里的应用图标拖进列表，状态会自动更新。',
)
const statusTitle = computed(() => {
  if (checking.value) return '正在检查…'
  if (granted.value) return '已授权'
  if (problem.value) return '无法确认'
  return '未授权'
})

function close() {
  emit('update:visible', false)
}

async function showDragPanel() {
  if (dragging.value || !appPath.value) return
  dragging.value = true
  try {
    await api.showFullDiskAccessDragPanel()
    if (!api.isTauri) Message.info('浏览器预览不会打开拖动面板')
  } catch (e) {
    Message.error(String(e))
  } finally {
    dragging.value = false
  }
}

async function reveal() {
  if (revealing.value || !appPath.value) return
  revealing.value = true
  try {
    await api.revealAppBundle()
    if (!api.isTauri) Message.info('浏览器预览不会打开访达')
  } catch (e) {
    Message.error(String(e))
  } finally {
    revealing.value = false
  }
}
</script>

<template>
  <a-modal
    :visible="visible"
    width="480px"
    title="完全磁盘访问权限"
    :mask-closable="true"
    @update:visible="(v) => emit('update:visible', v)"
  >
    <div class="fda-status" :class="{ 'is-granted': granted }">
      <span class="fda-dot" />
      <div class="fda-status-main">
        <span class="fda-status-title">{{ statusTitle }}</span>
        <span class="fda-status-text">
          <template v-if="granted">
            启动器现在可以读取 macOS 保护的文件。
          </template>
          <template v-else-if="problem">{{ problem }}</template>
          <template v-else>
            读取 macOS 保护的文件（例如其他应用的数据库）时会被系统拦下。
          </template>
        </span>
      </div>
      <button class="btn btn--quiet btn--sm" @click="emit('refresh')">
        重新检查
      </button>
    </div>

    <template v-if="!granted">
      <p class="fda-lead">{{ leadText }}</p>

      <button
        type="button"
        class="fda-app"
        :disabled="!appPath || dragging"
        :title="appPath || '当前构建没有可拖动的 .app 包'"
        @click="showDragPanel"
      >
        <img class="fda-app-icon" :src="launcherIcon" alt="" />
        <span class="fda-app-body">
          <span class="fda-app-name">DSH Launcher</span>
          <span class="fda-app-action">
            {{ appPath ? '打开拖动面板' : '当前构建没有可拖动的应用包' }}
          </span>
        </span>
      </button>

      <div v-if="!appPath" class="notice notice--warn fda-note">
        <div class="notice-body">
          开发构建没有 .app 包，无法拖动授权。请使用安装后的启动器操作。
        </div>
      </div>
      <p v-else class="fda-fallback">
        拖不过去时，点「在访达中显示」，从访达把应用拖进列表。
      </p>
    </template>

    <template #footer>
      <div class="fda-footer">
        <button
          class="btn btn--quiet"
          :disabled="!appPath || revealing"
          @click="reveal"
        >
          在访达中显示
        </button>
        <span class="fda-footer-gap" />
        <button class="btn btn--quiet" @click="close">关闭</button>
      </div>
    </template>
  </a-modal>
</template>

<style lang="scss" scoped>
.fda-status {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 12px 14px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-card);
  background: var(--dl-surface-2);

  &.is-granted {
    border-color: rgba(47, 125, 82, 0.28);
    background: var(--dl-run-soft);
  }
}

.fda-dot {
  width: 8px;
  height: 8px;
  margin-top: 6px;
  border-radius: 50%;
  background: var(--dl-ink-4);
  flex-shrink: 0;

  .is-granted & {
    background: var(--dl-run);
  }
}

.fda-status-main {
  display: flex;
  flex-direction: column;
  gap: 3px;
  flex: 1;
  min-width: 0;
}

.fda-status-title {
  color: var(--dl-ink);
  font-size: var(--dl-text-md);
  font-weight: 600;
}

.fda-status-text {
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
  line-height: 1.55;
}

.fda-lead {
  margin: 16px 0 12px;
  color: var(--dl-ink-2);
  font-size: var(--dl-text-md);
  line-height: 1.6;
}

.fda-app {
  display: flex;
  align-items: center;
  gap: 14px;
  width: 100%;
  padding: 12px 14px;
  border: 1px dashed var(--dl-line-strong);
  border-radius: var(--dl-r-card);
  background: var(--dl-surface-2);
  color: inherit;
  font-family: inherit;
  text-align: left;
  cursor: pointer;

  &:hover:not(:disabled) {
    border-color: var(--dl-accent-line);
    background: var(--dl-accent-soft);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.65;
  }
}

.fda-app-icon {
  width: 52px;
  height: 52px;
  flex-shrink: 0;
}

.fda-app-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.fda-app-name {
  color: var(--dl-ink);
  font-size: var(--dl-text-lg);
  font-weight: 600;
}

.fda-app-action {
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.fda-note {
  margin-top: 12px;
}

.fda-fallback {
  margin: 10px 0 0;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
  line-height: 1.55;
}

.fda-footer {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
}

.fda-footer-gap {
  flex: 1;
}
</style>

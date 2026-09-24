<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import type { TaskInfo } from '@/api/types'

const { t } = useI18n()
const store = useLauncherStore()

const expanded = ref<Record<string, boolean>>({})

interface ScrollbarExposed {
  scrollTo: (options: { top: number }) => void
}
const logRefs = ref<Record<string, ScrollbarExposed | null>>({})

onMounted(() => {
  store.refreshTasks()
})

function setLogRef(id: string, el: unknown) {
  logRefs.value[id] = (el as ScrollbarExposed | null) ?? null
}

async function scrollToBottom(id: string) {
  await nextTick()
  logRefs.value[id]?.scrollTo({ top: Number.MAX_SAFE_INTEGER })
}

watch(
  () => store.tasks,
  () => {
    for (const task of store.taskList) {
      if (task.state === 'running' && expanded.value[task.id] === undefined) {
        expanded.value[task.id] = true
      }
      if (expanded.value[task.id]) scrollToBottom(task.id)
    }
  },
  { deep: true },
)

function toggleExpand(id: string) {
  expanded.value[id] = !expanded.value[id]
  if (expanded.value[id]) scrollToBottom(id)
}

const cancelAction = useAction((id: string) => api.cancelTask(id), {
  key: (id) => id,
})
const onCancel = cancelAction.run

const removeAction = useAction(
  async (id: string) => {
    await api.removeTask(id)
    await store.refreshTasks()
  },
  { key: (id) => id },
)
const onRemove = removeAction.run

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString()
}

function instanceName(task: TaskInfo): string | null {
  if (!task.instance_id) return null
  return store.instanceById(task.instance_id)?.name ?? null
}

const sortedTasks = computed(() => store.taskList)
</script>

<template>
  <div class="dl-page tasks-page">
    <div class="dl-card tasks-card">
      <div class="dl-card-title">
        <div class="title-with-count">
          <h3>{{ t('tasks.title') }}</h3>
          <span v-if="store.runningTaskCount > 0" class="running-count-pill tnum">
            {{ store.runningTaskCount }}
          </span>
        </div>
        <div class="dl-toolbar">
          <button class="mac-secondary-btn" @click="store.refreshTasks()">
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
              <path d="M13.5 8A5.5 5.5 0 1 1 12 4.1L14 2" />
              <polyline points="14 5.5 14 2 10.5 2" />
            </svg>
            <span>{{ t('common.refresh') }}</span>
          </button>
        </div>
      </div>

      <div v-if="sortedTasks.length === 0" class="empty-tasks">
        <a-empty :description="t('tasks.empty')" />
      </div>

      <div v-for="task in sortedTasks" :key="task.id" class="task-box">
        <div class="task-head" @click="toggleExpand(task.id)">
          <div class="task-info">
            <div class="task-label">
              <span class="label-text">{{ task.label }}</span>
              <span :class="['apple-status-dot', task.state === 'running' ? 'running' : task.state === 'done' ? 'done' : task.state === 'error' ? 'error' : 'idle']">
                {{ t(`tasks.state.${task.state}`) }}
              </span>
            </div>
            <div class="task-meta tnum">
              {{ formatTime(task.created_at) }}
              <template v-if="task.state === 'done' && instanceName(task)">
                · {{ t('tasks.createdInstance', { name: instanceName(task) }) }}
              </template>
              <template v-if="task.state === 'error' && task.message"> · {{ task.message }}</template>
            </div>
          </div>

          <div class="task-progress">
            <a-progress
              v-if="task.state === 'running'"
              :percent="task.percent / 100"
              size="small"
              :show-text="true"
            />
            <a-progress
              v-else-if="task.state === 'done'"
              :percent="1"
              size="small"
              status="success"
            />
            <a-progress
              v-else
              :percent="task.percent / 100"
              size="small"
              :status="task.state === 'error' ? 'danger' : 'normal'"
            />
          </div>

          <div v-if="task.state === 'running' && task.percent >= 90" class="installing-hint">
            {{ t('tasks.installingHint') }}
          </div>

          <div class="task-actions" @click.stop>
            <button
              v-if="task.state === 'running'"
              class="mac-action-pill warning"
              @click="onCancel(task.id)"
            >
              {{ t('tasks.cancel') }}
            </button>
            <button
              v-else
              class="mac-action-pill danger"
              @click="onRemove(task.id)"
            >
              {{ t('tasks.remove') }}
            </button>
          </div>

          <span class="expand-icon" :class="{ open: expanded[task.id] }">
            <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M6 12l4-4-4-4" />
            </svg>
          </span>
        </div>

        <div v-if="expanded[task.id]" class="task-log-wrap">
          <a-scrollbar
            :ref="(el: unknown) => setLogRef(task.id, el)"
            class="task-log-scroller"
            style="max-height: 280px; overflow-y: auto"
          >
            <pre><code>{{ task.logs.join('\n') || t('tasks.noLogs') }}</code></pre>
          </a-scrollbar>
        </div>
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.title-with-count {
  display: flex;
  align-items: center;
  gap: 8px;

  .running-count-pill {
    padding: 1px 7px;
    font-size: 11px;
    font-weight: 600;
    border-radius: 10px;
    background: rgb(var(--primary-6) / 18%);
    color: rgb(var(--primary-6));
  }
}

.mac-secondary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 10px;
  font-size: 12.5px;
  font-weight: 500;
  border-radius: 7px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  cursor: pointer;
  transition: all 0.16s ease;

  &:hover {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.mac-action-pill {
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  border-radius: 6px;
  padding: 3px 9px;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.15s ease;

  &.warning {
    color: rgb(var(--orange-6));

    &:hover {
      background: rgb(var(--orange-6) / 12%);
    }
  }

  &.danger {
    color: rgb(var(--red-6));

    &:hover {
      background: rgb(var(--red-6) / 12%);
    }
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.empty-tasks {
  padding: 48px 0;
}

.task-box {
  border: 1px solid var(--apple-card-border);
  border-radius: 10px;
  margin-bottom: 12px;
  overflow: hidden;
  background: var(--apple-card-bg);
  transition: border-color 0.16s ease;
}

.task-head {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 12px 16px;
  cursor: pointer;
  user-select: none;
  transition: background 0.15s ease;

  &:hover {
    background: var(--apple-group-bg);
  }
}

.task-info {
  flex: 1;
  min-width: 0;
}

.task-label {
  display: flex;
  align-items: center;
  gap: 10px;

  .label-text {
    font-size: 13.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--color-text-1);
  }
}

.task-meta {
  font-size: 12px;
  color: var(--color-text-3);
  margin-top: 3px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.task-progress {
  width: 170px;
  flex-shrink: 0;
}

.installing-hint {
  flex-shrink: 0;
  font-size: 12px;
  font-weight: 500;
  color: rgb(var(--orange-6));
  white-space: nowrap;
}

.task-actions {
  flex-shrink: 0;
}

.expand-icon {
  color: var(--color-text-3);
  display: flex;
  align-items: center;
  transition: transform var(--apple-duration) var(--apple-spring-curve);

  &.open {
    transform: rotate(90deg);
  }
}

.task-log-wrap {
  border-top: 1px solid var(--apple-separator);
  background: #191b22;
}

.task-log-scroller {
  pre {
    margin: 0;
    padding: 14px 16px;
    font-family: 'SF Mono', Menlo, Monaco, Consolas, monospace;
    font-size: 12px;
    line-height: 1.6;
    color: #c9d1d9;
    white-space: pre-wrap;
    word-break: break-all;
  }
}
</style>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useLauncherStore } from '@/stores/launcher'
import { useCopy } from '@/composables/useCopy'
import { api } from '@/api'

const props = defineProps<{
  visible: boolean
  profileMap: Record<string, string | undefined>
}>()

const emit = defineEmits<{
  (e: 'update:visible', val: boolean): void
}>()

const { t } = useI18n()
const { copy } = useCopy()
const store = useLauncherStore()

const filter = ref<'all' | 'error' | 'warn'>('all')
const rechecking = ref(false)

const filteredLogs = computed(() => {
  if (filter.value === 'all') return store.healthLogs
  return store.healthLogs.filter((l) => l.level === filter.value)
})

function formatTime(ts: number): string {
  const d = new Date(ts)
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  const ss = String(d.getSeconds()).padStart(2, '0')
  return `${hh}:${mm}:${ss}`
}

async function onCopy() {
  if (store.healthLogs.length === 0) return
  const text = store.healthLogs
    .map((l) => {
      const time = formatTime(l.timestamp)
      const level = l.level.toUpperCase()
      return `[${time}] [${level}] [${l.instanceName} · ${l.profile}] ${l.message}`
    })
    .join('\n')
  await copy(text, 'home.copiedLogs')
}

function onClear() {
  store.clearHealthLogs()
}

async function onRecheck() {
  if (rechecking.value) return
  rechecking.value = true
  let foundAny = 0
  try {
    for (const inst of store.instances) {
      const profile = props.profileMap[inst.id] ?? inst.default_profile ?? 'web'
      try {
        const report = await api.checkInstanceHealth(inst.id, profile)
        if (report?.findings?.length) {
          foundAny += report.findings.length
          const newItems = report.findings.map((f, idx) => ({
            id: `${Date.now()}-${inst.id}-${idx}-${Math.random().toString(36).slice(2, 6)}`,
            timestamp: Date.now(),
            instanceId: inst.id,
            instanceName: inst.name,
            profile,
            level: f.level,
            code: f.code,
            message: f.message,
          }))
          store.addHealthLogs(newItems)
        }
      } catch {
        // Advisory check failure ignored
      }
    }
    if (foundAny > 0) {
      Message.warning(`${t('home.recheckHealth')}: 发现 ${foundAny} 项异常`)
    } else {
      Message.success(`${t('home.recheckHealth')}: ${t('home.noHealthLogs')}`)
    }
  } finally {
    rechecking.value = false
  }
}
</script>

<template>
  <a-modal
    :visible="visible"
    @update:visible="(val) => emit('update:visible', val)"
    :footer="false"
    :mask-closable="true"
    width="760px"
    modal-class="health-terminal-modal"
  >
    <template #title>
      <div class="terminal-modal-title">
        <svg class="terminal-modal-glyph" viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2.2" />
          <polyline points="4.6 6.2 6.6 8 4.6 9.8" />
          <line x1="8.2" y1="10" x2="11.4" y2="10" />
        </svg>
        <span class="terminal-title-text">{{ t('home.healthModalTitle') }}</span>
        <div class="terminal-summary-tags">
          <span v-if="store.healthErrorCount > 0" class="summary-badge error">
            {{ store.healthErrorCount }} {{ t('home.filterError') }}
          </span>
          <span v-if="store.healthWarnCount > 0" class="summary-badge warn">
            {{ store.healthWarnCount }} {{ t('home.filterWarn') }}
          </span>
        </div>
      </div>
    </template>

    <div class="terminal-container">
      <!-- 终端控制台顶部操作栏 -->
      <div class="terminal-toolbar">
        <div class="filter-group">
          <button
            class="filter-pill"
            :class="{ active: filter === 'all' }"
            @click="filter = 'all'"
          >
            {{ t('home.filterAll') }}
            <span class="pill-count">{{ store.healthTotalCount }}</span>
          </button>
          <button
            class="filter-pill pill-error"
            :class="{ active: filter === 'error' }"
            @click="filter = 'error'"
          >
            {{ t('home.filterError') }}
            <span class="pill-count" v-if="store.healthErrorCount > 0">{{ store.healthErrorCount }}</span>
          </button>
          <button
            class="filter-pill pill-warn"
            :class="{ active: filter === 'warn' }"
            @click="filter = 'warn'"
          >
            {{ t('home.filterWarn') }}
            <span class="pill-count" v-if="store.healthWarnCount > 0">{{ store.healthWarnCount }}</span>
          </button>
        </div>

        <div class="action-group">
          <button
            class="terminal-btn"
            :class="{ spinning: rechecking }"
            :disabled="rechecking"
            @click="onRecheck"
            :title="t('home.recheckHealth')"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
              <path d="M2.5 8a5.5 5.5 0 0 1 9.35-3.92L14 6" />
              <polyline points="10 6 14 6 14 2" />
              <path d="M13.5 8a5.5 5.5 0 0 1-9.35 3.92L2 10" />
              <polyline points="6 10 2 10 2 14" />
            </svg>
            <span>{{ t('home.recheckHealth') }}</span>
          </button>

          <button
            class="terminal-btn"
            :disabled="store.healthLogs.length === 0"
            @click="onCopy"
            :title="t('home.copyLogs')"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
              <rect x="5" y="5" width="8" height="8" rx="1.5" />
              <path d="M3 11V3h8" />
            </svg>
            <span>{{ t('home.copyLogs') }}</span>
          </button>

          <button
            class="terminal-btn btn-danger"
            :disabled="store.healthLogs.length === 0"
            @click="onClear"
            :title="t('home.clearLogs')"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
              <line x1="3" y1="4" x2="13" y2="4" />
              <path d="M5 4V3a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v1" />
              <path d="M6 7v5M10 7v5" />
              <rect x="4" y="4" width="8" height="9" rx="1" />
            </svg>
            <span>{{ t('home.clearLogs') }}</span>
          </button>
        </div>
      </div>

      <!-- 终端黑色日志输出界面 -->
      <div class="terminal-body">
        <div v-if="filteredLogs.length === 0" class="terminal-empty">
          <div class="empty-prompt">
            <span class="prompt-symbol">❯</span>
            <span class="prompt-cmd">deepseek-doctor --check</span>
          </div>
          <div class="empty-status-line">
            <span class="status-ok-icon">✓</span>
            <span class="status-ok-text">{{ t('home.noHealthLogs') }}</span>
          </div>
        </div>

        <div v-else class="terminal-lines">
          <div
            v-for="item in filteredLogs"
            :key="item.id"
            class="log-row"
            :class="`is-${item.level}`"
          >
            <div class="log-meta">
              <span class="meta-time">[{{ formatTime(item.timestamp) }}]</span>
              <span class="meta-level" :class="item.level">[{{ item.level.toUpperCase() }}]</span>
              <span class="meta-target">[{{ item.instanceName }} · {{ item.profile }}]</span>
            </div>
            <div class="log-content">{{ item.message }}</div>
          </div>
        </div>
      </div>
    </div>
  </a-modal>
</template>

<style lang="scss">
/* 穿透定制 Arco Modal 内部容器样式 */
.health-terminal-modal {
  .arco-modal-header {
    border-bottom: 1px solid var(--apple-card-border);
    padding: 12px 18px;
  }
  .arco-modal-body {
    padding: 14px 18px 18px;
    background: var(--apple-content-bg);
  }
}
</style>

<style lang="scss" scoped>
.terminal-modal-title {
  display: flex;
  align-items: center;
  gap: 10px;

  .terminal-modal-glyph {
    color: var(--color-text-3);
    flex-shrink: 0;
  }

  .terminal-title-text {
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text-1);
    letter-spacing: -0.01em;
  }

  .terminal-summary-tags {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 4px;

    .summary-badge {
      font-size: 11px;
      font-weight: 600;
      padding: 1px 7px;
      border-radius: 10px;

      &.error {
        background: rgba(239, 68, 68, 0.15);
        color: #ef4444;
      }
      &.warn {
        background: rgba(245, 158, 11, 0.15);
        color: #f59e0b;
      }
    }
  }
}

.terminal-container {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.terminal-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;

  .filter-group {
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--apple-card-bg);
    padding: 3px;
    border-radius: 8px;
    border: 1px solid var(--apple-card-border);

    .filter-pill {
      display: inline-flex;
      align-items: center;
      gap: 5px;
      padding: 3px 9px;
      font-size: 12px;
      font-weight: 500;
      border: none;
      background: transparent;
      color: var(--color-text-3);
      border-radius: 6px;
      cursor: pointer;
      transition: all 0.15s ease;

      .pill-count {
        font-size: 11px;
        opacity: 0.85;
      }

      &:hover {
        color: var(--color-text-1);
      }

      &.active {
        background: var(--apple-group-bg);
        color: var(--color-text-1);
        font-weight: 600;
        box-shadow: 0 1px 2px rgba(0, 0, 0, 0.05);
      }

      &.pill-error.active {
        color: #ef4444;
      }

      &.pill-warn.active {
        color: #f59e0b;
      }
    }
  }

  .action-group {
    display: flex;
    align-items: center;
    gap: 8px;

    .terminal-btn {
      display: inline-flex;
      align-items: center;
      gap: 5px;
      height: 28px;
      padding: 0 10px;
      font-size: 12px;
      font-weight: 500;
      border-radius: 7px;
      border: 1px solid var(--apple-card-border);
      background: var(--apple-card-bg);
      color: var(--color-text-2);
      cursor: pointer;
      transition: all 0.15s ease;

      &:hover:not(:disabled) {
        background: var(--apple-group-bg);
        color: var(--color-text-1);
      }

      &:disabled {
        opacity: 0.45;
        cursor: not-allowed;
      }

      &.btn-danger:hover:not(:disabled) {
        border-color: rgba(239, 68, 68, 0.3);
        color: #ef4444;
        background: rgba(239, 68, 68, 0.08);
      }

      &.spinning svg {
        animation: spin 1s linear infinite;
      }
    }
  }
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

.terminal-body {
  background: #15171c;
  color: #e2e8f0;
  border-radius: 10px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  box-shadow: inset 0 2px 6px rgba(0, 0, 0, 0.4);
  padding: 14px 16px;
  min-height: 240px;
  max-height: 420px;
  overflow-y: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace;
  font-size: 12.5px;
  line-height: 1.6;

  /* Custom scrollbar for dark terminal */
  &::-webkit-scrollbar {
    width: 6px;
  }
  &::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.18);
    border-radius: 3px;
  }

  .terminal-empty {
    padding: 24px 8px;
    display: flex;
    flex-direction: column;
    gap: 10px;

    .empty-prompt {
      display: flex;
      align-items: center;
      gap: 8px;

      .prompt-symbol {
        color: #10b981;
        font-weight: 700;
      }
      .prompt-cmd {
        color: #94a3b8;
      }
    }

    .empty-status-line {
      display: flex;
      align-items: center;
      gap: 8px;
      padding-left: 16px;

      .status-ok-icon {
        color: #10b981;
        font-weight: bold;
      }
      .status-ok-text {
        color: #64748b;
      }
    }
  }

  .terminal-lines {
    display: flex;
    flex-direction: column;
    gap: 8px;

    .log-row {
      padding: 8px 10px;
      border-radius: 6px;
      background: rgba(255, 255, 255, 0.03);
      border-left: 3px solid transparent;
      display: flex;
      flex-direction: column;
      gap: 4px;
      transition: background 0.12s ease;

      &:hover {
        background: rgba(255, 255, 255, 0.06);
      }

      &.is-error {
        border-left-color: #ef4444;
        background: rgba(239, 68, 68, 0.07);

        &:hover {
          background: rgba(239, 68, 68, 0.11);
        }
      }

      &.is-warn {
        border-left-color: #f59e0b;
        background: rgba(245, 158, 11, 0.07);

        &:hover {
          background: rgba(245, 158, 11, 0.11);
        }
      }

      .log-meta {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        gap: 8px;
        font-size: 11.5px;

        .meta-time {
          color: #64748b;
        }

        .meta-level {
          font-weight: 700;
          padding: 0 4px;
          border-radius: 3px;

          &.error {
            color: #f87171;
            background: rgba(248, 113, 113, 0.16);
          }

          &.warn {
            color: #fbbf24;
            background: rgba(251, 191, 36, 0.16);
          }
        }

        .meta-target {
          color: #60a5fa;
          font-weight: 500;
        }
      }

      .log-content {
        color: #f1f5f9;
        word-break: break-word;
        white-space: pre-wrap;
        font-size: 12px;
        line-height: 1.5;
      }
    }
  }
}
</style>

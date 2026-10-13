<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useLauncherStore } from '@/stores/launcher'
import { useCopy } from '@/composables/useCopy'
import { api } from '@/api'
import DlIcon from '@/components/DlIcon.vue'

// The dependency-tree preflight's findings, collected from every instance that
// has been checked. It is a console, so it stays a console: monospaced rows,
// severity as a left rule, and no chrome beyond the toolbar.

const props = defineProps<{
  visible: boolean
  profileMap: Record<string, string | undefined>
}>()

const emit = defineEmits<{ (e: 'update:visible', v: boolean): void }>()

const { t } = useI18n()
const { copy } = useCopy()
const store = useLauncherStore()

const filter = ref<'all' | 'error' | 'warn'>('all')
const rechecking = ref(false)

const logs = computed(() =>
  filter.value === 'all' ? store.healthLogs : store.healthLogs.filter((l) => l.level === filter.value),
)

function formatTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

async function onCopy() {
  if (store.healthLogs.length === 0) return
  const text = store.healthLogs
    .map(
      (l) =>
        `[${formatTime(l.timestamp)}] [${l.level.toUpperCase()}] [${l.instanceName} · ${l.profile}] ${l.message}`,
    )
    .join('\n')
  await copy(text, 'home.copiedLogs')
}

async function onRecheck() {
  if (rechecking.value) return
  rechecking.value = true
  let found = 0
  try {
    for (const inst of store.instances) {
      const profile = props.profileMap[inst.id] ?? inst.default_profile ?? 'web'
      try {
        const report = await api.checkInstanceHealth(inst.id, profile)
        if (!report?.findings?.length) continue
        found += report.findings.length
        store.addHealthLogs(
          report.findings.map((f, idx) => ({
            id: `${Date.now()}-${inst.id}-${idx}-${Math.random().toString(36).slice(2, 6)}`,
            timestamp: Date.now(),
            instanceId: inst.id,
            instanceName: inst.name,
            profile,
            level: f.level,
            code: f.code,
            message: f.message,
          })),
        )
      } catch {
        // Advisory check failure ignored
      }
    }
    if (found > 0) Message.warning(t('home.recheckFound', { count: found }))
    else Message.success(t('home.noHealthLogs'))
  } finally {
    rechecking.value = false
  }
}
</script>

<template>
  <a-modal
    :visible="visible"
    :footer="false"
    :mask-closable="true"
    width="720px"
    modal-class="dl-sheet"
    @update:visible="(v) => emit('update:visible', v)"
  >
    <template #title>
      <div class="health-title">
        <DlIcon name="health" :size="16" />
        <span>{{ t('home.healthModalTitle') }}</span>
        <span v-if="store.healthErrorCount > 0" class="dl-tag dl-tag--danger">
          {{ t('home.filterError') }} {{ store.healthErrorCount }}
        </span>
        <span v-if="store.healthWarnCount > 0" class="dl-tag dl-tag--warn">
          {{ t('home.filterWarn') }} {{ store.healthWarnCount }}
        </span>
      </div>
    </template>

    <div class="health-body">
      <div class="health-toolbar">
        <div class="dl-segmented">
          <button :class="{ 'is-active': filter === 'all' }" @click="filter = 'all'">
            {{ t('home.filterAll') }} <span class="dl-tnum">{{ store.healthTotalCount }}</span>
          </button>
          <button :class="{ 'is-active': filter === 'error' }" @click="filter = 'error'">
            {{ t('home.filterError') }} <span class="dl-tnum">{{ store.healthErrorCount }}</span>
          </button>
          <button :class="{ 'is-active': filter === 'warn' }" @click="filter = 'warn'">
            {{ t('home.filterWarn') }} <span class="dl-tnum">{{ store.healthWarnCount }}</span>
          </button>
        </div>

        <span class="health-spacer" />

        <button class="dl-btn dl-btn--quiet" :disabled="rechecking" @click="onRecheck">
          <DlIcon name="refresh" :size="13" />
          {{ t('home.recheckHealth') }}
        </button>
        <button class="dl-btn dl-btn--quiet" :disabled="store.healthLogs.length === 0" @click="onCopy">
          <DlIcon name="copy" :size="13" />
          {{ t('home.copyLogs') }}
        </button>
        <button
          class="dl-btn dl-btn--danger"
          :disabled="store.healthLogs.length === 0"
          @click="store.clearHealthLogs()"
        >
          {{ t('home.clearLogs') }}
        </button>
      </div>

      <div class="health-console">
        <div v-if="logs.length === 0" class="health-empty">
          <span class="health-empty-mark">✓</span>
          <span>{{ t('home.noHealthLogs') }}</span>
        </div>

        <div v-for="item in logs" :key="item.id" class="health-line" :class="`is-${item.level}`">
          <span class="health-meta dl-mono">
            {{ formatTime(item.timestamp) }} · {{ item.level.toUpperCase() }} ·
            {{ item.instanceName }} / {{ item.profile }}
          </span>
          <span class="health-msg">{{ item.message }}</span>
        </div>
      </div>
    </div>
  </a-modal>
</template>

<style lang="scss" scoped>
.health-title {
  display: flex;
  align-items: center;
  gap: 8px;
}

.health-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.health-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
}

.health-spacer {
  flex: 1;
}

// The console: dark in both themes, because a readout is a readout.
.health-console {
  max-height: 420px;
  min-height: 220px;
  overflow-y: auto;
  padding: 12px 14px;
  border-radius: var(--dl-r-card);
  background: #0f141c;
  color: #cbd5e1;
  font-family: var(--dl-font-mono);
  font-size: 12px;
  line-height: 1.6;
}

.health-empty {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 24px 4px;
  color: #64748b;

  .health-empty-mark {
    color: #3dd68c;
    font-weight: 700;
  }
}

.health-line {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 7px 10px;
  border-radius: 6px;
  border-left: 3px solid transparent;

  & + .health-line {
    margin-top: 6px;
  }

  &.is-warn {
    border-left-color: #e8a33d;
    background: rgba(232, 163, 61, 0.08);
  }

  &.is-error {
    border-left-color: #f0756a;
    background: rgba(240, 117, 106, 0.08);
  }
}

.health-meta {
  font-size: 11px;
  color: #64748b;
}

.health-msg {
  color: #e2e8f0;
  white-space: pre-wrap;
  word-break: break-word;
}
</style>

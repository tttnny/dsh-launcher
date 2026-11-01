<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Message, Modal } from '@arco-design/web-vue'
import { useLauncherStore } from '@/stores/launcher'
import { useCopy } from '@/composables/useCopy'
import { api } from '@/api'
import type { IconName } from '@/components/icons'
import DlIcon from '@/components/DlIcon.vue'

// The dependency-tree preflight's findings, collected from every instance that
// has been checked. It is a console, so it stays a console: monospaced rows,
// severity as a left rule, and no chrome beyond the toolbar.

const props = defineProps<{
  visible: boolean
  profileMap: Record<string, string | undefined>
}>()

const emit = defineEmits<{ (e: 'update:visible', v: boolean): void }>()

const { copy } = useCopy()
const store = useLauncherStore()

const filter = ref<'all' | 'error' | 'warn'>('all')
const rechecking = ref(false)
/**
 * What the latest 「重新自检」 did: the instances it covered, how many of them
 * it could not reach, and how many findings it added. This run is the only
 * source for 「没有发现异常」 — an empty list alone never means a clean tree.
 * The id snapshot keeps it honest: adding or removing an instance leaves the
 * result describing a board that no longer exists, so it is dropped.
 */
const lastCheck = ref<{
  total: number
  failed: number
  found: number
  instanceIds: string[]
} | null>(null)
const hasChecked = ref(false)

watch(
  () => store.instances.map((i) => i.id).join('|'),
  () => {
    lastCheck.value = null
  },
)

const logs = computed(() =>
  filter.value === 'all'
    ? store.healthLogs
    : store.healthLogs.filter((l) => l.level === filter.value),
)

/**
 * The real state behind an empty list. 「没有发现异常」 is reserved for a run
 * that reached every instance and found nothing; a filter with no rows, a list
 * that was never checked, and a run where machines could not be read each say
 * what they are.
 */
const emptyState = computed<{
  tone: string
  icon: IconName
  title: string
  text: string
}>(() => {
  const check = lastCheck.value
  // A run that could not reach machines is stated before the filter: it is a
  // fact about this check, not about which severity tab is open.
  if (check && check.total > 0 && check.failed > 0) {
    if (check.failed === check.total) {
      return {
        tone: 'danger',
        icon: 'alert',
        title: `全部 ${check.total} 个实例未能检查`,
        text: `检查请求没有返回结果，可能是主机不可达或实例没有运行。`,
      }
    }
    return {
      tone: 'warn',
      icon: 'alert',
      title: `本次有 ${check.failed} 个实例未能检查`,
      text: `其余 ${check.total - check.failed} 个实例没有发现异常。`,
    }
  }
  if (filter.value !== 'all') {
    return {
      tone: 'idle',
      icon: 'search',
      title: `当前筛选没有记录`,
      text: `切换到「全部」可以查看其它自检记录。`,
    }
  }
  if (check && check.total === 0) {
    return {
      tone: 'idle',
      icon: 'health',
      title: `没有可检查的实例`,
      text: `先创建并启动一个实例，再回来自检。`,
    }
  }
  if (check) {
    return {
      tone: 'ok',
      icon: 'check',
      title: `没有发现异常`,
      text: `所有实例的依赖都检查通过。`,
    }
  }
  if (!hasChecked.value) {
    return {
      tone: 'idle',
      icon: 'health',
      title: `尚未自检`,
      text: `点「重新自检」开始检查所有实例的依赖。`,
    }
  }
  return {
    tone: 'idle',
    icon: 'health',
    title: `还没有记录`,
    text: `点「重新自检」开始检查所有实例的依赖。`,
  }
})

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
  await copy(text, '已复制自检日志')
}

async function onRecheck() {
  if (rechecking.value) return
  rechecking.value = true
  const targets = [...store.instances]
  const total = targets.length
  let found = 0
  let failed = 0
  try {
    for (const inst of targets) {
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
        // A machine that did not answer is counted, never read as clean.
        failed += 1
      }
    }
    lastCheck.value = {
      total,
      failed,
      found,
      instanceIds: targets.map((i) => i.id),
    }
    hasChecked.value = total > 0
    if (total === 0) {
      Message.info(`没有可检查的实例`)
    } else if (failed === total) {
      Message.error(`全部 ${total} 个实例未能检查`)
    } else if (failed > 0) {
      Message.warning(
        found > 0
          ? `发现 ${found} 项异常，另有 ${failed} 个实例未能检查`
          : `${failed} 个实例未能检查，其余没有发现异常`,
      )
    } else if (found > 0) {
      Message.warning(`发现 ${found} 项异常`)
    } else {
      Message.success(`没有发现异常`)
    }
  } finally {
    rechecking.value = false
  }
}

function onClear() {
  Modal.confirm({
    title: `清空全部自检记录？`,
    content: `会删除当前所有实例的自检记录。清空后需要重新自检才能看到结果。`,
    okText: `清空`,
    cancelText: `取消`,
    okButtonProps: { status: 'danger' },
    modalClass: 'dl-sheet',
    onOk: () => {
      store.clearHealthLogs()
      lastCheck.value = null
      hasChecked.value = false
      Message.success(`已清空自检记录`)
    },
  })
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
        <span>{{ `依赖自检` }}</span>
        <span v-if="store.healthErrorCount > 0" class="dl-tag dl-tag--danger">
          {{ `错误` }} {{ store.healthErrorCount }}
        </span>
        <span v-if="store.healthWarnCount > 0" class="dl-tag dl-tag--warn">
          {{ `警告` }} {{ store.healthWarnCount }}
        </span>
      </div>
    </template>

    <div class="health-body">
      <div class="health-toolbar">
        <div class="dl-segmented">
          <button
            :class="{ 'is-active': filter === 'all' }"
            @click="filter = 'all'"
          >
            {{ `全部` }}
            <span class="dl-tnum">{{ store.healthTotalCount }}</span>
          </button>
          <button
            :class="{ 'is-active': filter === 'error' }"
            @click="filter = 'error'"
          >
            {{ `错误` }}
            <span class="dl-tnum">{{ store.healthErrorCount }}</span>
          </button>
          <button
            :class="{ 'is-active': filter === 'warn' }"
            @click="filter = 'warn'"
          >
            {{ `警告` }}
            <span class="dl-tnum">{{ store.healthWarnCount }}</span>
          </button>
        </div>

        <span class="health-spacer" />

        <button
          class="dl-btn dl-btn--quiet"
          :disabled="rechecking"
          @click="onRecheck"
        >
          <DlIcon name="refresh" :size="13" />
          {{ rechecking ? `检查中…` : `重新自检` }}
        </button>
        <button
          class="dl-btn dl-btn--quiet"
          :disabled="store.healthLogs.length === 0"
          @click="onCopy"
        >
          <DlIcon name="copy" :size="13" />
          {{ `复制` }}
        </button>
        <button
          class="dl-btn dl-btn--danger"
          :disabled="store.healthLogs.length === 0"
          @click="onClear"
        >
          {{ `清空` }}
        </button>
      </div>

      <!-- A run that left some machines unchecked says so above the rows, so
           the findings on screen are never mistaken for the whole picture. -->
      <p v-if="lastCheck && lastCheck.failed > 0" class="health-note">
        <DlIcon name="alert" :size="13" />
        {{ `本次自检有 ${lastCheck.failed} 个实例未能检查。` }}
      </p>

      <!-- An empty list gets a light, quiet state of its own: it is a statement
           about what was (not) checked, not a console with nothing in it. -->
      <div
        v-if="logs.length === 0"
        class="health-empty-state"
        :class="`is-${emptyState.tone}`"
      >
        <span class="health-empty-icon">
          <DlIcon :name="emptyState.icon" :size="22" />
        </span>
        <p class="health-empty-title">{{ emptyState.title }}</p>
        <p class="health-empty-text">{{ emptyState.text }}</p>
      </div>

      <div v-else class="health-console">
        <div
          v-for="item in logs"
          :key="item.id"
          class="health-line"
          :class="`is-${item.level}`"
        >
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

.health-note {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  padding: 8px 10px;
  border-radius: var(--dl-r-ctl);
  background: var(--dl-warn-soft);
  color: var(--dl-warn);
  font-size: var(--dl-text-sm);
}

// The console: dark in both themes, because a readout is a readout.
.health-console {
  max-height: 420px;
  min-height: 220px;
  overflow-y: auto;
  padding: 12px 14px;
  border-radius: var(--dl-r-card);
  background: var(--dl-readout-bg);
  color: var(--dl-readout-body);
  font-family: var(--dl-font-mono);
  font-size: 12px;
  line-height: 1.6;
}

// The light empty state: no dark block until there is something to read.
.health-empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 4px;
  min-height: 168px;
  padding: 28px 16px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-card);
  background: var(--dl-surface-2);
  text-align: center;
}

.health-empty-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 44px;
  margin-bottom: 4px;
  border-radius: 50%;
  background: var(--dl-surface);
  color: var(--dl-ink-3);
}

.health-empty-title {
  margin: 0;
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.health-empty-text {
  margin: 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.health-empty-state.is-ok .health-empty-icon {
  color: var(--dl-run);
}

.health-empty-state.is-warn .health-empty-icon {
  color: var(--dl-warn);
}

.health-empty-state.is-danger .health-empty-icon {
  color: var(--dl-danger);
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
    border-left-color: var(--dl-readout-warn);
    background: var(--dl-readout-warn-soft);
  }

  &.is-error {
    border-left-color: var(--dl-readout-danger);
    background: var(--dl-readout-danger-soft);
  }
}

.health-meta {
  font-size: 11px;
  color: var(--dl-readout-dim);
}

.health-msg {
  color: var(--dl-readout-ink);
  white-space: pre-wrap;
  word-break: break-word;
}
</style>

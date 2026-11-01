<script setup lang="ts">
import { computed } from 'vue'

// 一个状态灯加一句话。运行中、启动中、已停止、主机不可达的语义必须一眼分清，
// 主机不可达永远不能显示成已停止。

type Tone = 'run' | 'warn' | 'danger' | 'idle' | 'busy'

const props = defineProps<{
  state: string
  label?: string
  title?: string
}>()

const PRESET: Record<string, { label: string; tone: Tone; pulse?: boolean }> = {
  running: { label: '运行中', tone: 'run' },
  starting: { label: '启动中', tone: 'busy', pulse: true },
  stopped: { label: '已停止', tone: 'idle' },
  exited: { label: '已退出', tone: 'danger' },
  unreachable: { label: '主机不可达', tone: 'warn' },
  ok: { label: '正常', tone: 'run' },
  warn: { label: '需注意', tone: 'warn' },
  error: { label: '异常', tone: 'danger' },
  idle: { label: '未就绪', tone: 'idle' },
}

const preset = computed(
  () => PRESET[props.state] ?? { label: props.state, tone: 'idle' as Tone },
)
const label = computed(() => props.label ?? preset.value.label)
</script>

<template>
  <span class="status-badge" :class="'is-' + preset.tone" :title="title">
    <span class="status-dot" :class="{ 'is-pulse': preset.pulse }" />
    {{ label }}
  </span>
</template>

<style lang="scss" scoped>
.status-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
  font-weight: 500;
  white-space: nowrap;

  &.is-run {
    color: var(--dl-run);
  }

  &.is-warn,
  &.is-busy {
    color: var(--dl-warn);
  }

  &.is-danger {
    color: var(--dl-danger);
  }
}

.status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;

  &.is-pulse {
    animation: status-pulse 1.2s ease-in-out infinite;
  }
}

@keyframes status-pulse {
  50% {
    opacity: 0.32;
  }
}
</style>

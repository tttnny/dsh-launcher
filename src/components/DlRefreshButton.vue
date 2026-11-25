<script setup lang="ts">
import { ref } from 'vue'

const props = withDefaults(
  defineProps<{
    action: () => Promise<unknown>
    disabled?: boolean
    size?: number
  }>(),
  { size: 13, disabled: false },
)

const SPIN_MS = 700
const reduceMotion =
  window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
const running = ref(false)

async function onClick() {
  if (running.value || props.disabled) return
  running.value = true
  const started = performance.now()
  try {
    await props.action()
  } catch {
  } finally {
    if (!reduceMotion) {
      const elapsed = performance.now() - started
      const turns = Math.max(1, Math.ceil(elapsed / SPIN_MS))
      const wait = turns * SPIN_MS - elapsed
      if (wait > 0) await new Promise((resolve) => setTimeout(resolve, wait))
    }
    running.value = false
  }
}
</script>

<template>
  <button
    type="button"
    data-no-drag
    class="dl-refresh-btn"
    :class="{ 'is-busy': running }"
    :disabled="disabled"
    :aria-busy="running"
    :aria-disabled="disabled || running"
    @click="onClick"
  >
    <svg
      class="dl-refresh-glyph"
      :class="{ 'is-spinning': running }"
      viewBox="0 0 16 16"
      :width="size"
      :height="size"
      fill="none"
      stroke="currentColor"
      stroke-width="1.7"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M14 8a6 6 0 1 1-6-6c1.68 0 3.287.667 4.493 1.827L14 5.333" />
      <path d="M14 2v3.333h-3.333" />
    </svg>
    <slot />
  </button>
</template>

<style scoped>
.dl-refresh-glyph {
  display: block;
  transform-box: view-box;
  transform-origin: 50% 50%;
}

.dl-refresh-glyph.is-spinning {
  animation: dl-refresh-spin 700ms linear infinite;
}

.dl-refresh-btn.is-busy {
  cursor: default;
}

@keyframes dl-refresh-spin {
  from {
    transform: rotate(0deg);
  }

  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .dl-refresh-glyph.is-spinning {
    animation: none;
  }
}
</style>

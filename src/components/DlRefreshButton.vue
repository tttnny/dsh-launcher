<script setup lang="ts">
import { ref } from 'vue'

const props = withDefaults(
  defineProps<{
    /** Async work run on click; the glyph spins until it settles. */
    action: () => Promise<unknown>
    /** External busy/disabled condition (e.g. a store loading flag). */
    disabled?: boolean
    /** Glyph edge length in px. */
    size?: number
  }>(),
  { size: 13, disabled: false },
)

// One full glyph turn; keep in sync with the .is-spinning animation duration.
const SPIN_MS = 700
// Reduced-motion users get no spin, so holding the button for a full turn would
// read as a dead click rather than feedback: settle as soon as the work does.
const reduceMotion = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
const running = ref(false)

async function onClick() {
  if (running.value || props.disabled) return
  running.value = true
  const started = performance.now()
  try {
    await props.action()
  } catch {
    // Refresh is best-effort; callers surface their own errors.
  } finally {
    if (!reduceMotion) {
      // Always let the current turn finish, so the glyph stops upright instead
      // of snapping back from a random angle.
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
      <!-- A single ~300° arc with a clean corner barb: no wedge-cut ring, no
           arrowhead mushed into the stroke. -->
      <path d="M14 8a6 6 0 1 1-6-6c1.68 0 3.287.667 4.493 1.827L14 5.333" />
      <path d="M14 2v3.333h-3.333" />
    </svg>
    <slot />
  </button>
</template>

<style scoped>
/* Chrome (border, background, size) comes from the caller's button class, so
   the same glyph drops into the toolbar, a card, and a pill button. */
.dl-refresh-glyph {
  display: block;
  transform-box: view-box;
  transform-origin: 50% 50%;
}

.dl-refresh-glyph.is-spinning {
  animation: dl-refresh-spin 700ms linear infinite;
}

/* While working the button stays fully colored: dimming it would read as
   "unavailable", not "busy". Re-entrancy is guarded in onClick. */
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

<script setup lang="ts">
import { computed } from 'vue'

// 空状态永远是一个邀请：说明现在是什么情况，并给一个能继续走的动作。

type EmptyIcon = 'instance' | 'version' | 'data' | 'machine' | 'search'

const props = withDefaults(
  defineProps<{
    icon?: EmptyIcon
    title: string
    description?: string
  }>(),
  { icon: 'instance' },
)

const PATHS: Record<EmptyIcon, string> = {
  instance: 'M4 5.5h16v11H4zM8 9h8M8 12.5h5M2 9v6M22 9v6',
  version: 'M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3zM4 7.5l8 4.5 8-4.5M12 12v9',
  data: 'M5 4.5c0-1 3.1-1.8 7-1.8s7 .8 7 1.8-3.1 1.8-7 1.8-7-.8-7-1.8zM5 4.5v15c0 1 3.1 1.8 7 1.8s7-.8 7-1.8v-15M5 12c0 1 3.1 1.8 7 1.8s7-.8 7-1.8',
  machine: 'M3.5 4.5h17v6h-17zM3.5 13.5h17v6h-17zM7 7.5h.01M7 16.5h.01',
  search: 'M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM16 16l4.5 4.5',
}

const path = computed(() => PATHS[props.icon])
</script>

<template>
  <div class="empty">
    <span class="empty-glyph">
      <svg
        viewBox="0 0 24 24"
        width="24"
        height="24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path :d="path" />
      </svg>
    </span>
    <p class="empty-title">{{ title }}</p>
    <p v-if="description" class="empty-text">{{ description }}</p>
    <div v-if="$slots.default" class="empty-actions">
      <slot />
    </div>
  </div>
</template>

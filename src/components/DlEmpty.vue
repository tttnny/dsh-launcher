<script setup lang="ts">
import { computed } from 'vue'

// DlEmpty: the launcher's single empty-state visual.
//
// Every list in the app (versions, plugins, tasks, profiles, homes) used Arco's
// `a-empty`, whose stock "inbox" illustration and gray palette read as a
// different product. This renders one restrained macOS-style glyph instead:
// a hairline icon in the muted text color, then the message. Keeping it in one
// component means every empty state stays consistent by construction.

const props = withDefaults(
  defineProps<{
    /** Which glyph to draw. Defaults to a neutral tray. */
    icon?: 'tray' | 'search' | 'task' | 'profile' | 'plugin' | 'version'
    description?: string
  }>(),
  { icon: 'tray' },
)

const PATH = computed(() => {
  switch (props.icon) {
    case 'search':
      return 'M11 11l7 7M6.5 12a5.5 5.5 0 1 1 0-11 5.5 5.5 0 0 1 0 11z'
    case 'task':
      return 'M12 8v4l3 2M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z'
    case 'profile':
      return 'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4.5 20a7.5 7.5 0 0 1 15 0'
    case 'plugin':
      return 'M9 4.5a2 2 0 1 1 4 0V6h3.5a1 1 0 0 1 1 1v3.5h1.5a2 2 0 1 1 0 4H17.5V18a1 1 0 0 1-1 1H13v-1.5a2 2 0 1 0-4 0V19H5.5a1 1 0 0 1-1-1v-3.5H6a2 2 0 1 0 0-4H4.5V7a1 1 0 0 1 1-1H9V4.5z'
    case 'version':
      return 'M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3zM4 7.5l8 4.5 8-4.5M12 12v9'
    case 'tray':
    default:
      return 'M4 13.5V18a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4.5M4 13.5h4.5l1.5 2h4l1.5-2H20M4 13.5l2.2-7a1 1 0 0 1 .95-.7h9.7a1 1 0 0 1 .95.7L20 13.5'
  }
})
</script>

<template>
  <div class="dl-empty">
    <svg
      class="dl-empty-glyph"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path :d="PATH" />
    </svg>
    <p v-if="description" class="dl-empty-text">{{ description }}</p>
    <slot />
  </div>
</template>

<style lang="scss" scoped>
.dl-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 40px 20px;
  text-align: center;
}

.dl-empty-glyph {
  width: 34px;
  height: 34px;
  color: var(--color-text-4);
  opacity: 0.9;
}

.dl-empty-text {
  margin: 0;
  font-size: 13px;
  color: var(--color-text-3);
  max-width: 360px;
  line-height: 1.55;
}
</style>

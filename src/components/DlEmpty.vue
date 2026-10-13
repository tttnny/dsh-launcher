<script setup lang="ts">
// The app's single empty state: one hairline glyph, one title, one line of
// direction. It is an invitation to act, so it takes a default slot for the
// action button rather than leaving callers to bolt one on.

withDefaults(
  defineProps<{
    /** Which glyph to draw. */
    icon?: 'tray' | 'search' | 'profile' | 'plugin' | 'version'
    title?: string
    description?: string
  }>(),
  { icon: 'tray' },
)

const PATHS: Record<string, string> = {
  search: 'M11 11l7 7M6.5 12a5.5 5.5 0 1 1 0-11 5.5 5.5 0 0 1 0 11z',
  profile: 'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4.5 20a7.5 7.5 0 0 1 15 0',
  plugin:
    'M9 4.5a2 2 0 1 1 4 0V6h3.5a1 1 0 0 1 1 1v3.5h1.5a2 2 0 1 1 0 4H17.5V18a1 1 0 0 1-1 1H13v-1.5a2 2 0 1 0-4 0V19H5.5a1 1 0 0 1-1-1v-3.5H6a2 2 0 1 0 0-4H4.5V7a1 1 0 0 1 1-1H9V4.5z',
  version: 'M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3zM4 7.5l8 4.5 8-4.5M12 12v9',
  tray: 'M4 13.5V18a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4.5M4 13.5h4.5l1.5 2h4l1.5-2H20M4 13.5l2.2-7a1 1 0 0 1 .95-.7h9.7a1 1 0 0 1 .95.7L20 13.5',
}
</script>

<template>
  <div class="dl-empty">
    <svg
      class="dl-empty-glyph"
      viewBox="0 0 24 24"
      width="32"
      height="32"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path :d="PATHS[icon]" />
    </svg>
    <p v-if="title" class="dl-empty-title">{{ title }}</p>
    <p v-if="description" class="dl-empty-text">{{ description }}</p>
    <slot />
  </div>
</template>

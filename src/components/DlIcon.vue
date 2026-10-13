<script setup lang="ts">
import { FILLED, PATHS, type IconName } from './icons'

const props = withDefaults(
  defineProps<{
    name: IconName
    size?: number
  }>(),
  { size: 16 },
)

const filled = FILLED[props.name]
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    :fill="filled ? 'currentColor' : 'none'"
    stroke="currentColor"
    :stroke-width="filled ? 0 : 1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    v-html="filled ?? PATHS[name]"
  />
</template>

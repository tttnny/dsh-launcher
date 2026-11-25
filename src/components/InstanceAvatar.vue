<script setup lang="ts">
import { ref, watch } from 'vue'
import { api } from '@/api'
import launcherDefaultIcon from '@/assets/launcher-icon.png'

const props = withDefaults(
  defineProps<{ id: string; icon?: string | null; size?: number }>(),
  {
    size: 34,
  },
)

const src = ref<string | null>(null)

async function load() {
  if (!props.icon) {
    src.value = null
    return
  }
  try {
    src.value = await api.readInstanceIcon(props.id)
  } catch {
    src.value = null
  }
}

watch(() => [props.id, props.icon], load, { immediate: true })
</script>

<template>
  <span class="dl-avatar" :style="{ width: `${size}px`, height: `${size}px` }">
    <img :src="src ?? launcherDefaultIcon" alt="" />
  </span>
</template>

<style lang="scss" scoped>
.dl-avatar {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  overflow: hidden;
  border-radius: 8px;
  border: 1px solid var(--dl-line);
  background: var(--dl-surface-2);

  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
}
</style>

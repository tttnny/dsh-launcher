<script setup lang="ts">
import { ref, watch } from 'vue'
import { api } from '@/api'
import launcherDefaultIcon from '@/assets/launcher-icon.png'

// An instance's icon is either a custom image (URL or a cropped PNG in the
// HOME, both resolved backend-side) or the launcher mark. Loading lives here so
// the row and the detail header cannot disagree about which image to show.

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

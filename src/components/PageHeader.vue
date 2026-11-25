<script setup lang="ts">
import { useRouter } from 'vue-router'
import DlIcon from '@/components/DlIcon.vue'

const props = defineProps<{
  title: string
  description?: string
  backTo?: string
  backLabel?: string
}>()

const router = useRouter()

function goBack() {
  if (props.backTo) void router.push(props.backTo)
  else router.back()
}
</script>

<template>
  <header class="page-head">
    <div class="page-head-main">
      <button v-if="backTo !== undefined" class="page-back" @click="goBack">
        <DlIcon name="arrow-left" :size="13" />
        {{ backLabel ?? '返回' }}
      </button>
      <h1 class="page-title">{{ title }}</h1>
    </div>
    <div class="page-actions">
      <slot name="actions" />
    </div>
  </header>
</template>

<script setup lang="ts">
import type { DshInstance } from '@/api/types'

// 启动前的一次性选择：实例没有默认配置、而数据目录里有多个配置时，
// 直接问清楚这次用哪一个，不把选择藏在列表行里。

const props = defineProps<{
  visible: boolean
  instance: DshInstance | null
  profiles: string[]
}>()

const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'pick', profile: string): void
}>()

function choose(profile: string) {
  emit('pick', profile)
  emit('update:visible', false)
}
</script>

<template>
  <a-modal
    :visible="visible"
    width="400px"
    :title="`用哪个配置启动「${instance?.name ?? ''}」？`"
    @update:visible="(v) => emit('update:visible', v)"
  >
    <p class="picker-desc">
      这个实例还没有默认配置。选择这次启动使用的
      Profile；之后可以在实例设置里固定默认值。
    </p>
    <div class="picker-list">
      <button
        v-for="p in profiles"
        :key="p"
        class="picker-item"
        @click="choose(p)"
      >
        <span class="mono">{{ p }}</span>
      </button>
    </div>
    <template #footer>
      <button class="btn btn--quiet" @click="emit('update:visible', false)">
        取消
      </button>
    </template>
  </a-modal>
</template>

<style lang="scss" scoped>
.picker-desc {
  margin: 0 0 14px;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
  line-height: 1.6;
}

.picker-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.picker-item {
  display: flex;
  align-items: center;
  height: 38px;
  padding: 0 12px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface);
  color: var(--dl-ink);
  font-family: inherit;
  font-size: var(--dl-text-md);
  text-align: left;
  cursor: pointer;

  &:hover {
    border-color: var(--dl-accent-line);
    background: var(--dl-accent-soft);
  }
}
</style>

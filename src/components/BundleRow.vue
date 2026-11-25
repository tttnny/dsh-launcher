<script setup lang="ts">
import { Modal } from '@arco-design/web-vue'
import type { BundleItem } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'

const props = defineProps<{
  bundle: BundleItem
  pending: boolean
  managed: boolean
  locked?: boolean
  tone: 'own' | 'shipped'
}>()

const emit = defineEmits<{ toggle: [enabled: boolean]; remove: [] }>()

function name(): string {
  return props.bundle.title?.trim() || props.bundle.name
}

function bundleHasOwnTitle(): boolean {
  const title = props.bundle.title?.trim()
  return Boolean(title) && title !== props.bundle.name
}

function managedReason(): string | null {
  return props.bundle.readOnlyReason === 'management-required'
    ? '这个补丁层撑着 DSH 自己的插件管理，只能停用，不能移除'
    : null
}

function rowCount(): string | null {
  return props.bundle.rowCount > 0 ? `${props.bundle.rowCount} 个条目` : null
}

function confirmRemove() {
  Modal.confirm({
    title: `移除补丁层「${name()}」？`,
    content: `它带来的条目与配置会一起消失，这个 Profile 也不再依赖该包。本地 link 指向的源码目录不受影响。`,
    okText: '移除',
    cancelText: '取消',
    okButtonProps: { status: 'danger' },
    onOk: () => emit('remove'),
  })
}
</script>

<template>
  <div class="list-row bundle-row" :class="`is-${tone}`">
    <span
      class="bundle-dot"
      :class="{ 'is-on': bundle.enabled, 'is-bad': !!bundle.errorCode }"
    />

    <div class="row-main">
      <div class="row-title">
        <span class="bundle-name" :class="{ mono: !bundleHasOwnTitle() }">{{
          name()
        }}</span>
        <span v-if="bundle.version" class="tag tag--muted mono">{{
          bundle.version
        }}</span>
        <span
          v-if="managedReason()"
          class="tag tag--muted"
          :title="managedReason()!"
        >
          DSH 自身依赖
        </span>
        <span v-if="bundle.availability === 'missing'" class="tag tag--warn">
          包文件读不到
        </span>
        <span
          v-else-if="bundle.errorCode"
          class="tag tag--warn"
          :title="bundle.errorCode"
        >
          加载失败
        </span>
      </div>
    </div>

    <div class="row-actions">
      <a-switch
        :model-value="bundle.enabled"
        :loading="pending"
        :disabled="!managed || !!locked"
        size="small"
        @change="(v: string | number | boolean) => emit('toggle', v === true)"
      />
      <button
        v-if="managed && bundle.removable"
        class="btn btn--danger btn--sm"
        :disabled="pending || !!locked"
        @click="confirmRemove"
      >
        <DlIcon name="trash" :size="13" />
        移除
      </button>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.bundle-row {
  gap: 12px;
  min-height: 58px;
  padding: 11px 16px;
}

.bundle-row.is-shipped .bundle-name {
  font-weight: 500;
  color: var(--dl-ink-2);
}

.bundle-dot {
  width: 8px;
  height: 8px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--dl-ink-4);

  &.is-on {
    background: var(--dl-run);
  }

  &.is-bad {
    background: var(--dl-warn);
  }
}

.bundle-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.bundle-rows {
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-3);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
</style>

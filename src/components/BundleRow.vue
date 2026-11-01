<script setup lang="ts">
import { Modal } from '@arco-design/web-vue'
import type { BundleItem } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'

// 插件管理里的一层「补丁层」。它说明自己是什么（名称、版本、贡献多少条目），
// 并提供这个层仅有的两个动作：启用/停用，以及只对用户自己添加的层开放的移除。

const props = defineProps<{
  bundle: BundleItem
  pending: boolean
  managed: boolean
  /** 该页有任意写入进行中时，整页控件一起锁住。 */
  locked?: boolean
  /** 这一行属于哪一组，只影响视觉轻重。 */
  tone: 'own' | 'shipped'
}>()

const emit = defineEmits<{ toggle: [enabled: boolean]; remove: [] }>()

function name(): string {
  return props.bundle.title?.trim() || props.bundle.name
}

/** 只有当标题和包名不同时，包名才值得再占一行。 */
function bundleHasOwnTitle(): boolean {
  const title = props.bundle.title?.trim()
  return Boolean(title) && title !== props.bundle.name
}

/** 补丁层为什么没有「移除」按钮；DSH 自己的插件管理靠这几层撑着。 */
function managedReason(): string | null {
  return props.bundle.readOnlyReason === 'management-required'
    ? '这个补丁层撑着 DSH 自己的插件管理，只能停用，不能移除'
    : null
}

/** 没有条目的层是真实存在的（只装包或只做配置），不显示「0 个条目」。 */
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
        <!-- 展示的若就是包名，就用等宽字体：机器标识走读数面，翻译过的标题不走。 -->
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

// 折叠的「随 DSH 发布」那一组比你自己添加的层安静一些。
.bundle-row.is-shipped .bundle-name {
  font-weight: 500;
  color: var(--dl-ink-2);
}

// 颜色只说一件事：这一层现在是不是生效的。
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

// 层的规模：一个数字，不是一张列表 —— 里面的条目是 DSH 自己的组合细节。
.bundle-rows {
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-3);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
</style>

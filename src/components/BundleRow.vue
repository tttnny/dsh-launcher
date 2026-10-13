<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import type { BundleItem } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'

// One 补丁层 in the plugin manager's list.
//
// A layer states what it *is* (name, version, how many composition rows it
// contributes) and offers the two things a layer can do: mount/unmount, and —
// only for a layer the user added — 卸载. The three groups it can appear in
// (yours / ships with DSH / not mounted) are the page's business, so this row
// carries no group logic beyond the tone it is drawn with.

const props = defineProps<{
  bundle: BundleItem
  pending: boolean
  managed: boolean
  /** Which fold the row belongs to; only changes how loud it is. */
  tone: 'own' | 'shipped'
}>()

const emit = defineEmits<{ toggle: [enabled: boolean]; remove: [] }>()

const { t } = useI18n()

function name(): string {
  return props.bundle.title?.trim() || props.bundle.name
}

/** The package name only earns a second line when it is not what the title says. */
function hasOwnTitle(): boolean {
  const title = props.bundle.title?.trim()
  return Boolean(title) && title !== props.bundle.name
}

/** A layer with no rows is a real thing (an install-only or config-only
 *  layer); printing "0 条目" would be noise, so the fact stays absent. */
function rowCount(): string | null {
  return props.bundle.rowCount > 0
    ? t('plugins.bundleRows', { count: props.bundle.rowCount })
    : null
}
</script>

<template>
  <div class="bundle-row" :class="`is-${tone}`">
    <span class="bundle-dot" :class="{ 'is-on': bundle.enabled, 'is-bad': !!bundle.errorCode }" />
    <div class="bundle-main">
      <div class="bundle-title">
        <span class="bundle-name">{{ name() }}</span>
        <span v-if="bundle.version" class="dl-chip dl-mono">{{ bundle.version }}</span>
        <span v-if="rowCount()" class="bundle-rows">{{ rowCount() }}</span>
        <span v-if="bundle.optional" class="dl-tag dl-tag--muted">{{ t('plugins.bundleOptional') }}</span>
        <!-- "The package files are unreadable" already says the layer cannot
             load; the error code would be a second amber tag saying it twice. -->
        <span v-if="bundle.availability === 'missing'" class="dl-tag dl-tag--warn">
          {{ t('plugins.bundleMissing') }}
        </span>
        <span
          v-else-if="bundle.errorCode"
          class="dl-tag dl-tag--warn"
          :title="bundle.errorCode"
        >
          {{ t('plugins.bundleErrored') }}
        </span>
      </div>
      <span v-if="hasOwnTitle()" class="bundle-sub dl-mono">{{ bundle.name }}</span>
    </div>
    <div class="bundle-actions">
      <a-switch
        :model-value="bundle.enabled"
        :loading="pending"
        :disabled="!managed"
        size="small"
        @change="(v: string | number | boolean) => emit('toggle', v === true)"
      />
      <a-popconfirm
        v-if="managed && bundle.removable"
        :content="t('plugins.bundleRemoveConfirm', { name: name() })"
        :ok-text="t('plugins.bundleRemoveOk')"
        :cancel-text="t('common.cancel')"
        modal-class="dl-sheet"
        @ok="emit('remove')"
      >
        <button class="dl-btn dl-btn--danger" :disabled="pending">
          <DlIcon name="trash" :size="13" />
          {{ t('plugins.bundleRemove') }}
        </button>
      </a-popconfirm>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.bundle-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;
  border-top: 1px solid var(--dl-line);

  &:first-child {
    border-top: none;
  }
}

// The folded group sits quieter than the layers you added yourself.
.bundle-row.is-shipped {
  .bundle-name {
    font-weight: 500;
    color: var(--dl-ink-2);
  }
}

// Colour says one thing only: is this layer live right now.
.bundle-dot {
  width: 8px;
  height: 8px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--dl-ink-4);

  &.is-on {
    background: var(--dl-run);
    box-shadow: 0 0 0 3px var(--dl-run-soft);
  }

  &.is-bad {
    background: var(--dl-warn);
    box-shadow: 0 0 0 3px var(--dl-warn-soft);
  }
}

.bundle-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.bundle-title {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.bundle-name {
  font-size: var(--dl-text-md);
  font-weight: 550;
  color: var(--dl-ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

// The layer's size: a count, not a list — the rows are DSH's own detail.
.bundle-rows {
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-4);
  font-variant-numeric: tabular-nums;
}

.bundle-sub {
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-4);
  overflow: hidden;
  text-overflow: ellipsis;
}

.bundle-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}
</style>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { useLauncherStore } from '@/stores/launcher'
import { useProfiles } from '@/composables/useProfiles'
import { api } from '@/api'
import { useAction } from '@/composables/useAction'
import DlIcon from '@/components/DlIcon.vue'
import DlEmpty from '@/components/DlEmpty.vue'
import InstanceRow from '@/components/InstanceRow.vue'
import NewInstanceDialog from '@/components/NewInstanceDialog.vue'
import HealthLogModal from '@/components/HealthLogModal.vue'

const router = useRouter()
const route = useRoute()
const { t } = useI18n()
const store = useLauncherStore()
const { selectionByInstance } = useProfiles()

const query = ref('')
/**
 * The row the user was handed back to (`?focus=<id>`) — the plugin manager
 * sends this so "返回实例" lands on the instance they left, expanded.
 */
const focusId = computed(() => (typeof route.query.focus === 'string' ? route.query.focus : ''))
const newDialogVisible = ref(false)
/** Set when the sheet is opened as "duplicate this instance". */
const newPreset = ref<{ name: string; versionId: string; homeId: string } | null>(null)
const healthVisible = ref(false)

function openNewInstance() {
  newPreset.value = null
  newDialogVisible.value = true
}

function duplicateInstance(id: string) {
  const inst = store.instanceById(id)
  if (!inst) return
  newPreset.value = { name: inst.name, versionId: inst.version_id, homeId: inst.home_id }
  newDialogVisible.value = true
}

const runningCount = computed(
  () => store.instances.filter((i) => store.statusOf(i.id).state === 'running').length,
)

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  const list = !q
    ? store.instances
    : store.instances.filter(
        (i) => i.name.toLowerCase().includes(q) || i.id.toLowerCase().includes(q),
      )
  // Live instances float to the top: the board is a control surface, and the
  // thing you are controlling should be nearest your hand.
  return [...list].sort((a, b) => {
    const rank = (s: string) => (s === 'running' ? 0 : s === 'starting' ? 1 : 2)
    const d = rank(store.statusOf(a.id).state) - rank(store.statusOf(b.id).state)
    return d !== 0 ? d : a.name.localeCompare(b.name)
  })
})

// --- Start all / stop all -----------------------------------------------------

const startAllAction = useAction(async () => {
  const targets = store.instances.filter((i) => {
    const st = store.statusOf(i.id).state
    return st !== 'running' && st !== 'starting' && !!selectionByInstance[i.id]
  })
  await Promise.allSettled(
    targets.map((i) => api.startInstance(i.id, selectionByInstance[i.id]!)),
  )
  return targets.length
})

const stopAllAction = useAction(async () => {
  const targets = store.instances.filter((i) => store.statusOf(i.id).state === 'running')
  await Promise.allSettled(targets.map((i) => api.stopInstance(i.id)))
  return targets.length
})

const anyRunning = computed(() => runningCount.value > 0)
const anyStopped = computed(() => store.instances.length > runningCount.value)
</script>

<template>
  <div class="dl-page">
    <header class="dl-head">
      <div class="dl-head-text">
        <h1>{{ t('instances.pageTitle') }}</h1>
        <p class="dl-head-sub">{{ t('instances.pageDesc') }}</p>
      </div>
      <div class="dl-head-actions">
        <button
          v-if="anyStopped"
          class="dl-btn dl-btn--quiet"
          :disabled="startAllAction.busy['*']"
          @click="startAllAction.run()"
        >
          <DlIcon name="play" :size="13" />
          {{ t('instances.startAll') }}
        </button>
        <button
          v-if="anyRunning"
          class="dl-btn dl-btn--quiet"
          :disabled="stopAllAction.busy['*']"
          @click="stopAllAction.run()"
        >
          <DlIcon name="stop" :size="13" />
          {{ t('instances.stopAll') }}
        </button>
        <button class="dl-btn dl-btn--primary" @click="openNewInstance()">
          <DlIcon name="plus" :size="13" />
          {{ t('instances.newInstance') }}
        </button>
      </div>
    </header>

    <!-- Empty: the launcher has nothing to run yet. The only useful next step
         is installing a version, so the empty state says exactly that. -->
    <div v-if="store.instances.length === 0" class="dl-surface">
      <DlEmpty icon="version" :title="t('instances.emptyTitle')" :description="t('instances.emptyDesc')">
        <button class="dl-btn dl-btn--primary" @click="router.push({ name: 'library', query: { tab: 'versions' } })">
          <DlIcon name="download" :size="13" />
          {{ t('instances.emptyCta') }}
        </button>
      </DlEmpty>
    </div>

    <template v-else>
      <!-- Toolbar: search + a live summary. Kept above the list rather than in
           each row, so the board stays a table of facts. -->
      <div class="board-toolbar">
        <div class="dl-input dl-search board-search">
          <DlIcon name="search" :size="14" />
          <input v-model="query" class="dl-search-input" :placeholder="t('home.searchPlaceholder')" />
          <button v-if="query" class="dl-icon-btn dl-search-clear" @click="query = ''">
            <DlIcon name="close" :size="12" />
          </button>
        </div>

        <span class="board-summary dl-tnum">
          {{ t('instances.summary', { total: store.instances.length, running: runningCount }) }}
        </span>

        <span class="board-spacer" />

        <button
          class="dl-btn dl-btn--quiet board-health"
          :class="{
            'is-danger': store.healthErrorCount > 0,
            'is-warn': store.healthErrorCount === 0 && store.healthWarnCount > 0,
          }"
          @click="healthVisible = true"
        >
          <DlIcon name="health" :size="13" />
          {{ t('home.healthLogs') }}
          <span v-if="store.healthTotalCount > 0" class="board-health-count">
            {{ store.healthTotalCount > 99 ? '99+' : store.healthTotalCount }}
          </span>
        </button>
      </div>

      <div v-if="filtered.length === 0" class="dl-surface">
        <DlEmpty icon="search" :description="t('home.noMatchingInstances')" />
      </div>

      <div v-else class="dl-surface board">
        <InstanceRow
          v-for="inst in filtered"
          :key="inst.id"
          :inst="inst"
          :focus="inst.id === focusId"
          @changed="() => {}"
          @duplicate="duplicateInstance"
        />
      </div>
    </template>

    <NewInstanceDialog v-model:visible="newDialogVisible" :preset="newPreset" @created="() => {}" />
    <HealthLogModal v-model:visible="healthVisible" :profile-map="selectionByInstance" />
  </div>
</template>

<style lang="scss" scoped>
.board-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}

// Only the width is local: the field itself is `.dl-search` in index.scss.
.board-search {
  width: 260px;
}

.board-summary {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.board-spacer {
  flex: 1;
}

.board-health {
  &.is-warn {
    border-color: var(--dl-warn);
    color: var(--dl-warn);
  }

  &.is-danger {
    border-color: var(--dl-danger);
    color: var(--dl-danger);
  }
}

.board-health-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 17px;
  height: 17px;
  padding: 0 4px;
  border-radius: 9px;
  background: currentColor;
  color: var(--dl-surface);
  font-size: 10.5px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

.board {
  overflow: hidden;
}
</style>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { useLauncherStore } from '@/stores/launcher'
import { useProfiles } from '@/composables/useProfiles'
import { api } from '@/api'
import { bulkScope } from '@/instancesBulk'
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

/** The 全部 option of the host filter: a view, not a host. */
const HOST_ALL = 'all'

const query = ref('')
/**
 * The row the user was handed back to (`?focus=<id>`) — the plugin manager
 * sends this so "返回实例" lands on the instance they left, expanded.
 */
const focusId = computed(() => (typeof route.query.focus === 'string' ? route.query.focus : ''))
const newDialogVisible = ref(false)
/** Set when the sheet is opened as "duplicate this instance". */
const newPreset = ref<{ name: string; versionId: string; homeId: string; hostId?: string } | null>(
  null,
)
const healthVisible = ref(false)

function openNewInstance() {
  newPreset.value = null
  newDialogVisible.value = true
}

function duplicateInstance(id: string) {
  const inst = store.instanceById(id)
  if (!inst) return
  // The copy binds on the machine the original runs on, so the sheet lists that
  // machine's versions and DSH_HOMEs (an instance never spans two hosts).
  newPreset.value = {
    name: inst.name,
    versionId: inst.version_id,
    homeId: inst.home_id,
    hostId: inst.host_id,
  }
  newDialogVisible.value = true
}

/**
 * The host filter: 「全部」 or one machine, and picking a machine sets the one
 * 当前主机 the Library also reads — two places each keeping their own idea of
 * "which machine am I looking at" is what ADR-0006 §3 forbids.
 */
const hostFilter = computed({
  get: () => (store.allHosts ? HOST_ALL : store.currentHostId),
  set: (value: string) => {
    if (value === HOST_ALL) {
      store.setAllHosts(true)
      return
    }
    store.setAllHosts(false)
    void store.selectHost(value)
  },
})

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  const list = !q
    ? store.visibleInstances
    : store.visibleInstances.filter(
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

// --- What the two bulk buttons act on -----------------------------------------

/**
 * The visible rows, split by what each button would do to them. The rule lives
 * in `src/instancesBulk.ts` so the board and the buttons read one definition of
 * 「the range I am looking at」 — and so it can be driven without a webview
 * (`ci/instances-bulk.test.mjs`).
 */
const bulk = computed(() => bulkScope(filtered.value, (id) => store.statusOf(id)))

const runningCount = computed(() => bulk.value.stop.length)

// --- Start all / stop all -----------------------------------------------------

// They act on the rows the board is showing — a filter that says 「只看这台机器」
// and buttons that reach every machine would disagree with each other. One of
// those rows can also sit on a machine the launcher cannot reach at all, and
// 「主机不可达」 is not a kind of 「已停止」: nothing here may start it and have the
// user believe a second DSH is now running on that server (ADR-0007). Those rows
// are left alone and counted.

/**
 * What one bulk action did, in the numbers its toast states: how many went
 * through, how many failed — with the backend's own sentence for the first
 * one, so a refusal is not swallowed into `allSettled` — and how many rows were
 * deliberately left alone.
 */
interface BulkOutcome {
  done: number
  failed: number
  reason: string
  skipped: number
}

function summarize(results: PromiseSettledResult<unknown>[], skipped: number): BulkOutcome {
  const failed = results.filter((r): r is PromiseRejectedResult => r.status === 'rejected')
  return {
    done: results.length - failed.length,
    failed: failed.length,
    reason: failed.length ? String(failed[0].reason) : '',
    skipped,
  }
}

function bulkMessage(outcome: BulkOutcome, doneKey: string): string {
  const parts = [t(doneKey, { count: outcome.done })]
  if (outcome.failed) {
    parts.push(t('instances.bulkFailed', { count: outcome.failed, reason: outcome.reason }))
  }
  if (outcome.skipped) {
    parts.push(t('instances.startAllSkipped', { count: outcome.skipped }))
  }
  return parts.join('，')
}

const startAllAction = useAction(
  async () => {
    const targets = bulk.value.start.filter((i) => !!selectionByInstance[i.id])
    const results = await Promise.allSettled(
      targets.map((i) => api.startInstance(i.id, selectionByInstance[i.id]!)),
    )
    return summarize(results, bulk.value.unreachable.length)
  },
  { success: (outcome) => bulkMessage(outcome, 'instances.startAllDone') },
)

const stopAllAction = useAction(
  async () => {
    const results = await Promise.allSettled(bulk.value.stop.map((i) => api.stopInstance(i.id)))
    return summarize(results, 0)
  },
  { success: (outcome) => bulkMessage(outcome, 'instances.stopAllDone') },
)

const anyRunning = computed(() => bulk.value.stop.length > 0)
// Nothing startable in view means no button: an unreachable row has its own
// action (the row's 重试), and offering 全部启动 over it would promise an action
// the launcher is not going to take.
const anyStopped = computed(() => bulk.value.start.length > 0)
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
           each row, so the board stays a table of facts. The host filter sits
           with the search: both narrow what the board shows. -->
      <div class="board-toolbar">
        <div class="dl-input dl-search board-search">
          <DlIcon name="search" :size="14" />
          <input v-model="query" class="dl-search-input" :placeholder="t('home.searchPlaceholder')" />
          <button v-if="query" class="dl-icon-btn dl-search-clear" @click="query = ''">
            <DlIcon name="close" :size="12" />
          </button>
        </div>

        <span class="board-host-label">{{ t('instances.hostFilter') }}</span>
        <div class="dl-segmented board-hosts">
          <button :class="{ 'is-active': hostFilter === HOST_ALL }" @click="hostFilter = HOST_ALL">
            {{ t('instances.hostAll') }}
          </button>
          <button
            v-for="h in store.hosts"
            :key="h.id"
            :class="{ 'is-active': hostFilter === h.id }"
            :title="h.ssh_target || t('instances.localTag')"
            @click="hostFilter = h.id"
          >
            {{ h.display_name }}
          </button>
        </div>

        <span class="board-summary dl-tnum">
          {{ t('instances.summary', { total: filtered.length, running: runningCount }) }}
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

.board-host-label {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

// The host filter can carry several machine names; it wraps instead of pushing
// the summary and the health button off the row.
.board-hosts {
  flex-wrap: wrap;
  max-width: 46%;
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

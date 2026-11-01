<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useLauncherStore } from '@/stores/launcher'
import { useProfiles } from '@/composables/useProfiles'
import { api } from '@/api'
import { bulkScope } from '@/instancesBulk'
import { useAction } from '@/composables/useAction'
import type { DshInstance } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'
import DlEmpty from '@/components/DlEmpty.vue'
import InstanceRow from '@/components/InstanceRow.vue'
import NewInstanceDialog from '@/components/NewInstanceDialog.vue'
import HealthLogModal from '@/components/HealthLogModal.vue'

const router = useRouter()
const route = useRoute()
const store = useLauncherStore()
const { selectionByInstance, ensureSelection, profilesError } = useProfiles()

const query = ref('')
/**
 * The row the user was handed back to (`?focus=<id>`) — the plugin manager
 * sends this so "返回实例" lands on the instance they left, expanded.
 */
const focusId = computed(() =>
  typeof route.query.focus === 'string' ? route.query.focus : '',
)
const newDialogVisible = ref(false)
/** Set when the sheet is opened as "duplicate this instance". */
const newPreset = ref<{
  name: string
  versionId: string
  homeId: string
  hostId?: string
} | null>(null)
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

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  const list = !q
    ? store.visibleInstances
    : store.visibleInstances.filter(
        (i) =>
          i.name.toLowerCase().includes(q) || i.id.toLowerCase().includes(q),
      )
  return [...list].sort((a, b) => a.name.localeCompare(b.name))
})

/**
 * The board, grouped by what the rows actually are. State is the one axis a
 * launcher is read along, so it becomes the page's structure rather than a
 * sort order: live channels first (nearest the hand), then the resting ones,
 * then the machines that did not answer — never folded into 已停止, because
 * unknown is not stopped.
 */
const groups = computed(() => {
  const running: DshInstance[] = []
  const stopped: DshInstance[] = []
  const unreachable: DshInstance[] = []
  for (const inst of filtered.value) {
    const state = store.statusOf(inst.id).state
    if (state === 'running' || state === 'starting') running.push(inst)
    else if (state === 'unreachable') unreachable.push(inst)
    else stopped.push(inst)
  }
  return [
    { key: 'running', label: '运行中', items: running },
    { key: 'stopped', label: '已停止', items: stopped },
    { key: 'unreachable', label: '主机不可达', items: unreachable },
  ].filter((g) => g.items.length > 0)
})

// --- What the two bulk buttons act on -----------------------------------------

/**
 * The visible rows, split by what each button would do to them. The rule lives
 * in `src/instancesBulk.ts` so the board and the buttons read one definition of
 * 「the range I am looking at」 — and so it can be driven without a webview
 * (`ci/instances-bulk.test.mjs`).
 */
const bulk = computed(() =>
  bulkScope(filtered.value, (id) => store.statusOf(id)),
)

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
  /** Rows that had no usable Profile and were therefore not started. */
  noProfile: number
  /** Rows whose Profile list could not be read at all. */
  readFailed: number
}

function summarize(
  results: PromiseSettledResult<unknown>[],
  skipped: number,
  noProfile = 0,
  readFailed = 0,
): BulkOutcome {
  const failed = results.filter(
    (r): r is PromiseRejectedResult => r.status === 'rejected',
  )
  return {
    done: results.length - failed.length,
    failed: failed.length,
    reason: failed.length ? String(failed[0].reason) : '',
    skipped,
    noProfile,
    readFailed,
  }
}

function bulkMessage(outcome: BulkOutcome, doneVerb: string): string {
  const parts = [`${doneVerb} ${outcome.done} 个实例`]
  if (outcome.failed) {
    parts.push(`${outcome.failed} 个失败（${outcome.reason}）`)
  }
  if (outcome.readFailed) {
    parts.push(`${outcome.readFailed} 个实例读取 Profile 失败，未启动`)
  }
  if (outcome.noProfile) {
    parts.push(`${outcome.noProfile} 个实例没有可用 Profile，未启动`)
  }
  if (outcome.skipped) {
    parts.push(`${outcome.skipped} 个在不可达的主机上，未动`)
  }
  return parts.join('，')
}

const startAllAction = useAction(
  async () => {
    // Every startable row gets its Profile resolved first; a row without one is
    // counted and reported instead of being dropped without a word.
    const started: Promise<unknown>[] = []
    let noProfile = 0
    let readFailed = 0
    for (const inst of bulk.value.start) {
      const profile = await ensureSelection(inst)
      if (!profile) {
        if (profilesError[inst.home_id]) readFailed++
        else noProfile++
        continue
      }
      started.push(api.startInstance(inst.id, profile))
    }
    const results = await Promise.allSettled(started)
    return summarize(
      results,
      bulk.value.unreachable.length,
      noProfile,
      readFailed,
    )
  },
  { success: (outcome) => bulkMessage(outcome, '已启动') },
)

const stopAllAction = useAction(
  async () => {
    const results = await Promise.allSettled(
      bulk.value.stop.map((i) => api.stopInstance(i.id)),
    )
    return summarize(results, 0)
  },
  { success: (outcome) => bulkMessage(outcome, '已停止') },
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
        <h1>{{ `实例` }}</h1>
      </div>
      <div class="dl-head-actions">
        <button
          v-if="anyStopped"
          class="dl-btn dl-btn--quiet"
          :disabled="startAllAction.busy['*']"
          @click="startAllAction.run()"
        >
          <DlIcon name="play" :size="13" />
          {{ `全部启动` }}
        </button>
        <button
          v-if="anyRunning"
          class="dl-btn dl-btn--quiet"
          :disabled="stopAllAction.busy['*']"
          @click="stopAllAction.run()"
        >
          <DlIcon name="stop" :size="13" />
          {{ `全部停止` }}
        </button>
        <button class="dl-btn dl-btn--primary" @click="openNewInstance()">
          <DlIcon name="plus" :size="13" />
          {{ `新建实例` }}
        </button>
      </div>
    </header>

    <!-- Empty: the launcher has nothing to run yet. The only useful next step
         is installing a version, so the empty state says exactly that. -->
    <div v-if="store.instances.length === 0" class="dl-surface">
      <DlEmpty
        icon="version"
        :title="`还没有实例`"
        :description="`安装一个 DSH 版本会自动创建它的第一个实例；也可以在这里为已安装的版本再建一个。`"
      >
        <button
          class="dl-btn dl-btn--primary-soft"
          @click="router.push({ name: 'library', query: { tab: 'versions' } })"
        >
          <DlIcon name="download" :size="13" />
          {{ `安装 DSH 版本` }}
        </button>
      </DlEmpty>
    </div>

    <template v-else>
      <!-- The board's own toolbar: a filter for the rows in view, and the
           self-check console. 主机 lives in the rail, so it is not repeated here. -->
      <div class="board-toolbar">
        <div class="dl-input dl-search board-search">
          <DlIcon name="search" :size="14" />
          <input
            v-model="query"
            class="dl-search-input"
            :placeholder="`搜索实例名称`"
          />
          <button
            v-if="query"
            class="dl-icon-btn dl-search-clear"
            @click="query = ''"
          >
            <DlIcon name="close" :size="12" />
          </button>
        </div>

        <span class="board-spacer" />

        <button
          class="dl-btn dl-btn--quiet board-health"
          :class="{
            'is-danger': store.healthErrorCount > 0,
            'is-warn':
              store.healthErrorCount === 0 && store.healthWarnCount > 0,
          }"
          @click="healthVisible = true"
        >
          <DlIcon name="health" :size="13" />
          {{ `自检` }}
          <span v-if="store.healthTotalCount > 0" class="board-health-count">
            {{ store.healthTotalCount > 99 ? '99+' : store.healthTotalCount }}
          </span>
        </button>
      </div>

      <div v-if="filtered.length === 0" class="dl-surface">
        <DlEmpty icon="search" :description="`没有匹配的实例`" />
      </div>

      <template v-else>
        <section v-for="g in groups" :key="g.key" class="board-group">
          <div class="board-group-head">
            <span class="board-group-dot" :class="'is-' + g.key" />
            <span class="board-group-title">{{ g.label }}</span>
            <span class="board-group-count">{{ g.items.length }}</span>
          </div>
          <div class="dl-surface board">
            <InstanceRow
              v-for="inst in g.items"
              :key="inst.id"
              :inst="inst"
              :focus="inst.id === focusId"
              @changed="() => {}"
              @duplicate="duplicateInstance"
            />
          </div>
        </section>
      </template>
    </template>

    <NewInstanceDialog
      v-model:visible="newDialogVisible"
      :preset="newPreset"
      @created="() => {}"
    />
    <HealthLogModal
      v-model:visible="healthVisible"
      :profile-map="selectionByInstance"
    />
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
  width: 300px;
}

// --- Board groups -----------------------------------------------------------
// A quiet group head over each band of channels: the state is the structure,
// not a colour on a row.

.board-group + .board-group {
  margin-top: 22px;
}

.board-group-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 0 8px;
  padding-left: 2px;
}

.board-group-dot {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--dl-ink-4);

  &.is-running {
    background: var(--dl-run);
  }

  &.is-unreachable {
    background: var(--dl-warn);
  }
}

.board-group-title {
  font-size: var(--dl-text-sm);
  font-weight: 600;
  color: var(--dl-ink-2);
}

.board-group-count {
  font-size: var(--dl-text-xs);
  font-weight: 600;
  color: var(--dl-ink-3);
  font-variant-numeric: tabular-nums;
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

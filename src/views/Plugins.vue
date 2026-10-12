<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import type { BundleItem, ChangeOutcome, PluginItem, PluginView } from '@/api/types'
import DlEmpty from '@/components/DlEmpty.vue'

const { t } = useI18n()
const store = useLauncherStore()

// --- The (HOME, Profile) it manages -------------------------------------------
// The card is driven by its parent — the Profile row it was opened from — and
// never picks its own subject: the Profile is the row the route named, and the
// HOME is the list's. Both are props, so the manager cannot quietly show one
// Profile while the page above it highlights another.

const props = defineProps<{ profile: string; homeId: string }>()

const emit = defineEmits<{ close: [] }>()

const selectedHomeId = computed(() => props.homeId)
const selectedProfile = computed(() => props.profile)

const selectedHome = computed(() => store.homeById(props.homeId))

// --- The plugin view of (selectedHomeId, selectedProfile) ---------------------
// One snapshot from the running instance's native plugin manager. `mode`
// decides whether the page manages or only displays; `reason` is shown
// verbatim (the backend localizes it).
//
// The Profile identifier is part of every row: a Profile can sit under several
// DSH_HOME directories, and the plugin manager always asks one (HOME, Profile)
// pair, so the pair is what the header states and what the rows are read from.

const pluginView = ref<PluginView | null>(null)
const pluginsLoading = ref(false)

const plugins = computed<PluginItem[]>(() => pluginView.value?.plugins ?? [])
const viewMode = computed(() => pluginView.value?.mode ?? 'not-running')
const viewReason = computed(() => pluginView.value?.reason ?? null)
const managed = computed(() => viewMode.value === 'managed')
// When the read-only list came from the profile's own files, the UI says so:
// the file-sourced list is a best-effort projection, not the instance's answer.
const viewFiles = computed(() => pluginView.value?.files ?? false)
const filesNote = computed(() =>
  viewFiles.value && plugins.value.length > 0 ? t('plugins.readonlyFilesNote') : '',
)

// --- Status filter ------------------------------------------------------------
// The absolute list is what the instance answered; the filter only narrows what
// is on screen, so a hidden row is never mistaken for a missing one (the count
// on each chip is the real one).

type StatusFilter = 'all' | 'on' | 'off' | 'problem'

const statusFilter = ref<StatusFilter>('all')

const problemCount = computed(() => plugins.value.filter(isProblem).length)

/** A row the instance cannot address through the profile patch. `management-required`
 *  entries (the manager's own core entries) are normal, not problems. */
function isProblem(p: PluginItem): boolean {
  return p.readOnlyReason === 'unaddressable'
}

const statusCounts = computed(() => ({
  all: plugins.value.length,
  on: plugins.value.filter((p) => p.enabled).length,
  off: plugins.value.filter((p) => !p.enabled).length,
  problem: problemCount.value,
}))

const statusFilters = computed(() => {
  const counts = statusCounts.value
  return [
    { key: 'all' as const, label: t('plugins.filterAll'), count: counts.all },
    { key: 'on' as const, label: t('plugins.filterOn'), count: counts.on },
    { key: 'off' as const, label: t('plugins.filterOff'), count: counts.off },
    // Only offered when something is actually wrong: a chip that always reads 0
    // is noise on the majority of profiles.
    ...(counts.problem > 0
      ? [{ key: 'problem' as const, label: t('plugins.filterProblem'), count: counts.problem }]
      : []),
  ]
})

const visiblePlugins = computed<PluginItem[]>(() => {
  switch (statusFilter.value) {
    case 'on':
      return plugins.value.filter((p) => p.enabled)
    case 'off':
      return plugins.value.filter((p) => !p.enabled)
    case 'problem':
      return plugins.value.filter(isProblem)
    default:
      return plugins.value
  }
})

// A narrowed list the user then changes Profile on should not stay filtered —
// the chips' counts would no longer describe what is on screen.
watch(statusFilter, (v) => {
  if (v !== 'all' && pluginView.value === null) statusFilter.value = 'all'
})

/** Display name: the instance's localized title, falling back to the module. */
function pluginName(p: PluginItem): string {
  return p.title?.trim() || p.moduleName
}

async function loadPlugins() {
  pluginView.value = null
  selectedEntries.value = []
  if (!selectedHomeId.value || !selectedProfile.value) return
  pluginsLoading.value = true
  try {
    pluginView.value = await api.listPlugins(selectedHomeId.value, selectedProfile.value)
  } catch (e) {
    Message.error(String(e))
  } finally {
    pluginsLoading.value = false
  }
}

// The card is mounted once per open Profile, so the first read is the mount,
// and a later change of either prop is a switch to another Profile's manager.
//
// `onMounted` rather than `{ immediate: true }` on the watcher: an immediate
// watcher runs inside `setup()`, before the parent has finished mounting, and
// its `await` then lands outside the render — the fetch resolved in a later
// tick than the mount effect's first run, and the table painted once, empty,
// and never updated. Mounting is the one hook that reliably precedes the first
// paint.
onMounted(() => {
  void loadPlugins()
})

watch([selectedHomeId, selectedProfile], () => {
  void loadPlugins()
})

// --- Enable / disable one plugin entry through the instance's native API ------
// The toggle sends the entry's `entryId`; the outcome is reported truthfully
// (applied / restart-required / overridden / failed / cancelled), never a fixed
// "restart to take effect". Batch enable/disable goes through the same path.

const selectedEntries = ref<string[]>([])

/** How an outcome message should be announced. */
type OutcomeTone = 'success' | 'warning' | 'info'

/** Tone + label for one `application` value (five possible, not three). */
function applicationOutcome(application: string): { tone: OutcomeTone; label: string } {
  switch (application) {
    case 'applied':
      return { tone: 'success', label: t('plugins.outcomeApplied') }
    case 'restart-required':
      return { tone: 'success', label: t('plugins.outcomeRestartRequired') }
    case 'overridden':
      return { tone: 'warning', label: t('plugins.outcomeOverridden') }
    case 'failed':
      return { tone: 'warning', label: t('plugins.outcomeFailed') }
    case 'cancelled':
      return { tone: 'info', label: t('plugins.outcomeCancelled') }
    default:
      return { tone: 'warning', label: t('plugins.outcomeUnknown', { application }) }
  }
}

/**
 * The one renderer for a write outcome, shared by the plugin-entry path, the
 * bundle toggle and the bundle removal (design §4b/§4e), so the refusal branch
 * and the five `application` branches can never drift between them.
 *
 * A refusal (`changed:false`) is read from the **localized** reason the backend
 * folders into `diagnostic`; `errorCode` is the raw native code and is never
 * shown (a missing diagnostic falls back to a graceful generic sentence, never
 * a bare code). `success` carries only the per-call-site success wording that
 * genuinely differs (a bundle says "已更新补丁层…", an entry says "已生效").
 */
function renderOutcome(
  o: ChangeOutcome,
  success?: { applied: string; restartRequired: string },
): { tone: OutcomeTone; message: string } {
  if (!o.changed) {
    const reason = o.diagnostic?.trim()
    // `errorCode` is a raw native code and is never shown; with no localized
    // reason to show, fall back to the generic refusal sentence, not a code.
    return {
      tone: 'warning',
      message: reason ? t('plugins.outcomeRefused', { reason }) : t('plugins.outcomeBatchRefused'),
    }
  }
  const { tone, label } = applicationOutcome(o.application)
  if (o.application === 'applied' && success) return { tone, message: success.applied }
  if (o.application === 'restart-required' && success) {
    return { tone, message: success.restartRequired }
  }
  // A `failed` outcome carries the instance's own diagnostic when it has one.
  if (o.application === 'failed') {
    const detail = o.diagnostic?.trim()
    if (detail) return { tone, message: `${label}：${detail}` }
  }
  return { tone, message: label }
}

/** Shows one rendered outcome at the tone it deserves. */
function showOutcome({ tone, message }: { tone: OutcomeTone; message: string }) {
  if (tone === 'success') Message.success(message)
  else if (tone === 'info') Message.info(message)
  else Message.warning(message)
}

async function onTogglePlugin(record: PluginItem, enabled: boolean) {
  const homeId = selectedHomeId.value
  const profile = selectedProfile.value
  if (!homeId || !profile || !managed.value) return
  const outcomes = await toggleAction.run(homeId, profile, [{ id: record.entryId, enabled }])
  if (outcomes?.length) reportOutcomes([{ name: pluginName(record), outcome: outcomes[0] }])
  // Success, refusal or a transport failure all leave the list re-synced to
  // the instance's true state.
  await loadPlugins()
}

async function applyBatch(enabled: boolean) {
  const homeId = selectedHomeId.value
  const profile = selectedProfile.value
  const keys = [...selectedEntries.value]
  if (!homeId || !profile || !managed.value || keys.length === 0) return
  const items = keys.map((id) => ({ id, enabled }))
  const outcomes = await toggleAction.run(homeId, profile, items)
  if (outcomes?.length) {
    reportOutcomes(
      outcomes.map((outcome, i) => ({
        name: plugins.value.find((p) => p.entryId === keys[i])?.moduleName ?? keys[i],
        outcome,
      })),
    )
  }
  selectedEntries.value = []
  await loadPlugins()
}

/**
 * Reports what the instance actually did. Success, refusal and "changed but did
 * not take effect" are all separate answers — a single fixed hint would be
 * wrong (the official web template hot-reloads).
 */
function reportOutcomes(results: { name: string; outcome: ChangeOutcome }[]) {
  if (results.length === 1) {
    const { name, outcome } = results[0]
    const { tone, message } = renderOutcome(outcome)
    const text = t('plugins.outcomeFor', { name, message })
    if (tone === 'success') Message.success(text)
    else if (tone === 'info') Message.info(text)
    else Message.warning(text)
    return
  }
  // A batch may mix results; count each answer instead of flattening them.
  const summary = results.reduce<Record<string, number>>((acc, r) => {
    const key = r.outcome.changed ? r.outcome.application : 'refused'
    acc[key] = (acc[key] ?? 0) + 1
    return acc
  }, {})
  const parts = Object.entries(summary).map(([key, count]) => {
    const label =
      key === 'refused' ? t('plugins.outcomeBatchRefused') : applicationOutcome(key).label
    return `${label} ${count}`
  })
  Message.info(t('plugins.outcomeBatch', { total: results.length, parts: parts.join('，') }))
}

const toggleAction = useAction(
  async (homeId: string, profile: string, items: { id: string; enabled: boolean }[]) => {
    // Sequential: the batch reuses the single-entry write path, so every item
    // is one native call and the instance's per-entry answers stay truthful.
    const outcomes: ChangeOutcome[] = []
    for (const item of items) {
      outcomes.push(
        await api.setPluginEnabled({ homeId, profile, id: item.id, enabled: item.enabled }),
      )
    }
    return outcomes
  },
)

// Every plugin control is disabled while any toggle is in flight (the busy
// semantics the page already had).
const pluginsBusy = computed(() => toggleAction.busy['*'] === true)

// Once the instance starts (or stops), the page must switch between managed and
// read-only without an app restart. The backend resolves the HOME+Profile to a
// running instance, so a change to any instance status of the selected HOME is
// what re-reads the view.
const statusTick = computed(() => {
  if (!selectedHomeId.value) return ''
  return store.instances
    .filter((i) => i.home_id === selectedHomeId.value)
    .map((i) => {
      const st = store.statusOf(i.id)
      return `${i.id}:${st.state}:${st.profile ?? ''}`
    })
    .join('|')
})
watch(statusTick, (now, before) => {
  if (now !== before) void loadPlugins()
})

// --- Patch layers (bundles) --------------------------------------------------
// A separate layer from the plugin entries below: a bundle is a whole patch
// layer (GLOSSARY: Bundle（补丁层）) that mounts/unmounts as a unit, while a
// plugin entry is one addressable node in the composition tree. The two switch
// the instance's native manager through different methods and mean different
// things, so they live in separate cards with their own labels.

const bundles = computed<BundleItem[]>(() => pluginView.value?.bundles ?? [])
// Mounted layers first (in the instance's order = priority), then the rest.
const mountedBundles = computed(() => bundles.value.filter((b) => b.enabled))
const availableBundles = computed(() => bundles.value.filter((b) => !b.enabled))
const showAvailable = ref(false)
// Per-bundle in-flight flag so a slow toggle only disables its own row.
const bundlePending = ref<Record<string, boolean>>({})

/** Display name: the instance's localized title, falling back to the name. */
function bundleName(b: BundleItem): string {
  return b.title?.trim() || b.name
}

/** One bundle toggle. Refusals come back as an outcome, not a thrown error. */
async function onToggleBundle(b: BundleItem, enabled: boolean) {
  if (!selectedHomeId.value || !selectedProfile.value) return
  bundlePending.value = { ...bundlePending.value, [b.name]: true }
  try {
    const outcome = await api.setBundleEnabled({
      homeId: selectedHomeId.value,
      profile: selectedProfile.value,
      name: b.name,
      enabled,
    })
    // The shared renderer folds in the localized refusal reason (diagnostic) and
    // the five application branches; only the success wording is bundle-specific.
    showOutcome(
      renderOutcome(outcome, {
        applied: t('plugins.bundleApplied', { name: bundleName(b) }),
        restartRequired: t('plugins.bundleAppliedRestart', { name: bundleName(b) }),
      }),
    )
    // Re-sync: the entries the layer brought in appear/disappear, and the
    // order reflects the instance's list (a re-mounted layer is appended last).
    await loadPlugins()
  } catch (e) {
    // Transport-layer failure: surface it and re-sync so the UI is not stale.
    Message.error(String(e))
    await loadPlugins()
  } finally {
    const next = { ...bundlePending.value }
    delete next[b.name]
    bundlePending.value = next
  }
}

/**
 * Whether a removable bundle's delete entry is now actionable: only while the
 * profile is manageable and this row has no write in flight. A bundle that
 * ships with DSH (`removable:false`) never qualifies.
 */
function canRemove(b: BundleItem): boolean {
  return managed.value && b.removable && !bundlePending.value[b.name]
}

/**
 * Why a bundle that ships with DSH (`removable:false`) cannot be removed,
 * shown as a tooltip on its disabled delete button. This is the **common**
 * case, not an edge case: every bundle shipped with the install reports
 * `removable:false`, including the base and web-app layers.
 */
function removeBlockedReason(b: BundleItem): string {
  return b.readOnlyReason === 'management-required'
    ? t('plugins.bundleRemoveManaged')
    : t('plugins.bundleRemoveNotRemovable')
}

/**
 * Removes a whole Bundle（补丁层） through the instance's native plugin manager
 * — the same path the Web UI uses, so a plugin the Web UI can remove the
 * launcher can too. `removable:false` bundles never reach here (their button is
 * disabled); the outcome is still reported truthfully if the instance refuses.
 */
async function onRemoveBundle(b: BundleItem) {
  if (!selectedHomeId.value || !selectedProfile.value) return
  bundlePending.value = { ...bundlePending.value, [b.name]: true }
  try {
    const outcome = await api.removeBundle({
      homeId: selectedHomeId.value,
      profile: selectedProfile.value,
      name: b.name,
    })
    // Same shared renderer as the toggle and the plugin-entry path: the
    // localized refusal reason lives in `diagnostic`, the five application
    // branches live in one place, and only the success wording differs.
    showOutcome(
      renderOutcome(outcome, {
        applied: t('plugins.bundleRemoved', { name: bundleName(b) }),
        restartRequired: t('plugins.bundleRemovedRestart', { name: bundleName(b) }),
      }),
    )
    // Re-sync so the list matches the instance's true state — the bundle and
    // the entries it brought in must really be gone.
    await loadPlugins()
  } catch (e) {
    // Transport-layer failure: the removal did not happen; surface it and
    // re-sync so the UI is not stale.
    Message.error(String(e))
    await loadPlugins()
  } finally {
    const next = { ...bundlePending.value }
    delete next[b.name]
    bundlePending.value = next
  }
}

const pluginColumns = computed(() => [
  { title: t('plugins.pluginName'), slotName: 'pname', width: 260 },
  { title: t('plugins.pluginEntry'), slotName: 'pentry', ellipsis: true, tooltip: true },
  { title: t('plugins.pluginPhase'), slotName: 'pphase', width: 110 },
  { title: t('plugins.pluginStatus'), slotName: 'pstatus', width: 130 },
])
</script>

<template>
  <!-- The subject is the parent's: this card renders one (HOME, Profile) and
       never another, so it opens on the row's own section instead of a page
       heading plus a second set of selectors. -->
  <section class="plugin-manager">
    <div class="plugin-manager-head">
      <h3>
        {{ t('plugins.pageTitle') }}
        <span class="plugin-manager-scope tnum">{{ selectedHome?.name }} / {{ selectedProfile }}</span>
      </h3>
      <button class="mac-secondary-btn" @click="emit('close')">{{ t('plugins.close') }}</button>
    </div>
    <p class="plugin-manager-desc">{{ t('plugins.pageDesc') }}</p>

    <div v-if="!selectedHomeId || !selectedProfile" class="dl-card">
      <DlEmpty icon="plugin" :description="t('plugins.pluginsPickProfile')" />
    </div>

    <template v-else>
      <!-- Plugins Card — the primary object of this page, so it comes first. -->
      <div class="dl-card entries-card">
        <div class="dl-card-title">
          <div class="title-with-pill">
            <h3>{{ t('plugins.pluginsTitle') }}</h3>
            <span class="layer-tag entry-layer-tag">{{ t('plugins.entriesLayerTag') }}</span>
          </div>
        <!-- Batch enable/disable: the same native path as a single toggle. Only
             meaningful while the instance is manageable. -->
        <div v-if="managed && plugins.length > 0" class="batch-btns">
          <button
            class="mac-action-pill"
            :disabled="selectedEntries.length === 0 || pluginsBusy"
            @click="applyBatch(true)"
          >
            {{ t('plugins.pluginsBatchEnable', { count: selectedEntries.length }) }}
          </button>
          <button
            class="mac-action-pill"
            :disabled="selectedEntries.length === 0 || pluginsBusy"
            @click="applyBatch(false)"
          >
            {{ t('plugins.pluginsBatchDisable', { count: selectedEntries.length }) }}
          </button>
        </div>
      </div>
      <p class="dl-card-desc">{{ t('plugins.pluginsDesc') }}</p>

      <!-- Read-only banner: no instance running this profile, adopted instance,
           or an unreachable instance. The reason is backend-localized. -->
      <div v-if="!managed && viewReason" class="plugins-readonly-banner">
        {{ viewReason }}
      </div>

      <!-- Status filter: chips carry the real counts (absolute, never filtered
           counts), so a narrowed list is never mistaken for a shorter one. -->
      <div v-if="plugins.length > 0" class="filter-chips">
        <button
          v-for="f in statusFilters"
          :key="f.key"
          class="filter-chip"
          :class="{ active: statusFilter === f.key }"
          type="button"
          @click="statusFilter = f.key"
        >
          {{ f.label }}
          <span class="filter-chip-count tnum">{{ f.count }}</span>
        </button>
      </div>

      <!-- The table renders whenever the instance gave us entries (managed), or
           the offline read produced a file-sourced list; a pure read-only page
           with nothing to show has only the banner, not a contradictory empty
           table. -->
      <a-table
        v-if="managed || plugins.length > 0"
        :columns="pluginColumns"
        :data="visiblePlugins"
        :loading="pluginsLoading"
        :pagination="false"
        row-key="entryId"
        :row-selection="
          managed
            ? {
                type: 'checkbox',
                showCheckedAll: true,
                onlyCurrent: true,
                selectedRowKeys: selectedEntries,
              }
            : undefined
        "
        class="apple-styled-table"
        @selection-change="(keys: (string | number)[]) => (selectedEntries = keys.map(String))"
      >
        <template #pname="{ record }">
          <span class="plugin-cell-name">{{ pluginName(record) }}</span>
          <!-- Identity line: the module the entry loads, then its version.
               Two versions are the same vocabulary as the patch-layer card
               below — a plugin entry simply has no version of its own, so it
               is the version of the bundle that provided it. -->
          <span class="plugin-cell-sub">
            <span
              v-if="record.title?.trim() && record.title.trim() !== record.moduleName"
              class="plugin-cell-module tnum"
            >{{ record.moduleName }}</span>
            <span v-if="record.version" class="dl-chip tnum">{{ record.version }}</span>
          </span>
        </template>
        <template #pentry="{ record }">
          <span class="plugin-cell-id tnum">{{ record.entryId }}</span>
        </template>
        <template #pphase="{ record }">
          <span v-if="record.fiberPhase" class="plugin-phase">{{ record.fiberPhase }}</span>
          <span v-else class="plugin-no-version">-</span>
        </template>
        <template #pstatus="{ record }">
          <!-- The instance owns the state: the toggle is wired to the native
               writer. The switch reflects the instance's value (the row is
               re-synced after every write, and on failure too). -->
          <a-switch
            :model-value="record.enabled"
            :disabled="!managed || pluginsBusy"
            :checked-text="t('plugins.pluginOn')"
            :unchecked-text="t('plugins.pluginOff')"
            size="small"
            @change="(v: string | number | boolean) => onTogglePlugin(record, v === true)"
          />
        </template>
        <template #empty>
          <DlEmpty
            :icon="statusFilter === 'all' ? 'plugin' : 'search'"
            :description="
              statusFilter === 'all'
                ? t('plugins.pluginsEmpty')
                : t('plugins.pluginsFilteredEmpty')
            "
          />
        </template>
      </a-table>
      <!-- Read-only list read from the profile files: the offline content is a
           best-effort projection, so say how it differs from the instance's. -->
      <p v-if="filesNote" class="plugins-files-note">{{ filesNote }}</p>
    </div>

    <!-- Patch layers (bundles) card — a layer ABOVE the plugin entries. The
         switch here is a bundle-layer switch; the switch in the entry table is
         a single-plugin-entry switch. It sits below the entries because it is
         the rarer operation, and its own heading carries the distinction. -->
    <div
      v-if="selectedHomeId && selectedProfile && (managed || bundles.length > 0)"
      class="dl-card bundles-card"
    >
      <div class="dl-card-title">
        <div class="title-with-pill">
          <h3>{{ t('plugins.bundlesTitle') }}</h3>
          <span class="layer-tag bundle-layer-tag">{{ t('plugins.bundlesLayerTag') }}</span>
        </div>
      </div>
      <p class="dl-card-desc">{{ t('plugins.bundlesDesc') }}</p>
      <p class="bundle-order-hint">{{ t('plugins.bundleOrderHint') }}</p>

      <!-- Mounted layers, in the instance's order (top = highest priority). -->
      <div v-if="mountedBundles.length" class="bundle-group">
        <div class="bundle-group-label">{{ t('plugins.bundlesMounted') }}</div>
        <div v-for="b in mountedBundles" :key="b.name" class="bundle-row">
          <div class="bundle-row-main">
            <div class="bundle-row-head">
              <span class="bundle-row-name">{{ bundleName(b) }}</span>
              <span v-if="b.version" class="dl-chip">{{ b.version }}</span>
              <span v-if="b.optional" class="bundle-chip">{{ t('plugins.bundleOptional') }}</span>
              <span v-if="!b.removable" class="bundle-chip bundle-chip-muted">{{ t('plugins.bundleReadonly') }}</span>
              <span v-if="b.availability === 'missing'" class="bundle-chip bundle-chip-warn">{{ t('plugins.bundleMissing') }}</span>
            </div>
            <div class="bundle-row-name-sub tnum">{{ b.name }}</div>
            <div class="bundle-row-state">{{ t('plugins.bundleStateActive') }}</div>
          </div>
          <div class="bundle-row-actions">
            <a-switch
              :model-value="b.enabled"
              :loading="!!bundlePending[b.name]"
              :checked-text="t('plugins.bundleOn')"
              :unchecked-text="t('plugins.bundleOff')"
              size="small"
              @change="(value) => onToggleBundle(b, Boolean(value))"
            />
            <!-- Removal goes through the instance's native API. A bundle that
                 ships with DSH (`removable:false`) is the common case: the
                 button is disabled and its reason is shown on hover. -->
            <a-popconfirm
              v-if="canRemove(b)"
              :content="t('plugins.bundleRemoveConfirm', { name: bundleName(b) })"
              :ok-text="t('plugins.bundleRemoveOk')"
              @ok="onRemoveBundle(b)"
            >
              <button class="mac-micro-btn danger">{{ t('plugins.bundleRemove') }}</button>
            </a-popconfirm>
            <!-- A bundle that ships with DSH cannot be removed at all: the
                 button is disabled and its reason is shown. A disabled button
                 receives no hover events, so the reason rides a wrapper the
                 tooltip can bind to. -->
            <a-tooltip v-else-if="!b.removable" :content="removeBlockedReason(b)">
              <span class="bundle-remove-disabled">
                <button class="mac-micro-btn danger" disabled>{{ t('plugins.bundleRemove') }}</button>
              </span>
            </a-tooltip>
            <!-- Removable, but a write for this row is in flight. -->
            <button v-else class="mac-micro-btn danger" disabled>
              {{ t('plugins.bundleRemove') }}
            </button>
          </div>
        </div>
      </div>
      <div v-else class="bundle-empty">{{ t('plugins.bundlesEmpty') }}</div>

      <!-- Available (disabled) layers — most bundles ship disabled, so they
           stay collapsed and never bury the active layers above. -->
      <div v-if="availableBundles.length" class="bundle-group">
        <button class="bundle-toggle" type="button" @click="showAvailable = !showAvailable">
          {{ showAvailable
            ? t('plugins.bundlesHideAvailable')
            : t('plugins.bundlesShowAvailable', { count: availableBundles.length }) }}
        </button>
        <template v-if="showAvailable">
          <div v-for="b in availableBundles" :key="b.name" class="bundle-row bundle-row-off">
            <div class="bundle-row-main">
              <div class="bundle-row-head">
                <span class="bundle-row-name">{{ bundleName(b) }}</span>
                <span v-if="b.version" class="dl-chip">{{ b.version }}</span>
                <span v-if="b.optional" class="bundle-chip">{{ t('plugins.bundleOptional') }}</span>
                <span v-if="!b.removable" class="bundle-chip bundle-chip-muted">{{ t('plugins.bundleReadonly') }}</span>
                <span v-if="b.availability === 'missing'" class="bundle-chip bundle-chip-warn">{{ t('plugins.bundleMissing') }}</span>
              </div>
              <div class="bundle-row-name-sub tnum">{{ b.name }}</div>
              <div class="bundle-row-state">{{ t('plugins.bundleStateInactive') }}</div>
            </div>
            <div class="bundle-row-actions">
              <a-switch
                :model-value="b.enabled"
                :loading="!!bundlePending[b.name]"
                :disabled="b.availability === 'missing'"
                :checked-text="t('plugins.bundleOn')"
                :unchecked-text="t('plugins.bundleOff')"
                size="small"
                @change="(value) => onToggleBundle(b, Boolean(value))"
              />
              <a-popconfirm
                v-if="canRemove(b)"
                :content="t('plugins.bundleRemoveConfirm', { name: bundleName(b) })"
                :ok-text="t('plugins.bundleRemoveOk')"
                @ok="onRemoveBundle(b)"
              >
                <button class="mac-micro-btn danger">{{ t('plugins.bundleRemove') }}</button>
              </a-popconfirm>
              <a-tooltip v-else-if="!b.removable" :content="removeBlockedReason(b)">
                <span class="bundle-remove-disabled">
                  <button class="mac-micro-btn danger" disabled>{{ t('plugins.bundleRemove') }}</button>
                </span>
              </a-tooltip>
              <button v-else class="mac-micro-btn danger" disabled>
                {{ t('plugins.bundleRemove') }}
              </button>
            </div>
          </div>
        </template>
      </div>
    </div>
    </template>
  </section>
</template>

<style lang="scss" scoped>
// The manager is not a page: it is a section of the Profile list, opened from
// the row it manages. It therefore carries a section heading rather than a page
// heading, and its parts are spaced like the cards around it.
.plugin-manager {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.plugin-manager-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 8px;
  padding-top: 18px;
  border-top: 1px solid var(--apple-separator);

  h3 {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin: 0;
    font-size: 17px;
    font-weight: 600;
    letter-spacing: -0.015em;
    color: var(--color-text-1);
  }
}

.plugin-manager-scope {
  font-size: 12px;
  font-weight: 500;
  color: var(--color-text-3);
}

.plugin-manager-desc {
  margin: -6px 0 0;
  max-width: 68ch;
  font-size: 13px;
  line-height: 1.55;
  color: var(--color-text-3);
}

.title-with-pill {
  display: flex;
  align-items: center;
  gap: 10px;

  h3 {
    margin: 0;
  }
}

.batch-btns {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

/* Status filter chips: one row, counts always absolute. */
.filter-chips {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  margin-bottom: 14px;
}

.filter-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 999px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.15s ease;

  .filter-chip-count {
    font-size: 11px;
    color: var(--color-text-3);
  }

  &:hover {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &.active {
    border-color: rgba(var(--primary-6), 0.35);
    background: rgba(var(--primary-6), 0.1);
    color: rgb(var(--primary-6));

    .filter-chip-count {
      color: rgb(var(--primary-6));
    }
  }
}

/* Layer tag: tells the user which kind of switch a card holds — a whole
   patch layer (bundle) vs a single plugin entry. */
.layer-tag {
  display: inline-flex;
  align-items: center;
  font-size: 11px;
  font-weight: 600;
  padding: 1px 7px;
  border-radius: 999px;
  border: 1px solid transparent;
}

.bundle-layer-tag {
  color: #ad6800;
  background: #fff7e6;
  border-color: #ffe7ba;
}

.entry-layer-tag {
  color: var(--color-text-3);
  background: var(--apple-group-bg);
  border-color: var(--apple-card-border);
}

.bundles-card .bundle-order-hint {
  margin: -4px 0 14px;
  font-size: 12px;
  color: var(--color-text-3);
}

.bundle-group {
  & + .bundle-group {
    margin-top: 14px;
  }
}

.bundle-group-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text-2);
  margin-bottom: 8px;
}

.bundle-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 10px 12px;
  border-radius: 8px;
  background: var(--apple-group-bg);
  border: 1px solid var(--apple-card-border);

  & + .bundle-row {
    margin-top: 8px;
  }
}

.bundle-row-off {
  opacity: 0.82;
}

.bundle-row-main {
  min-width: 0;
}

/* Row controls: the mount/unmount switch and the remove button. Kept in one
   group so a row stays a single line whatever the bundle's removable state. */
.bundle-row-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;

  /* A disabled remove button must *read* as disabled — the shared
     .mac-micro-btn keeps its colour when disabled, so dim it here. */
  .mac-micro-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
}

/* The tooltip binds to this wrapper: a disabled button emits no hover events. */
.bundle-remove-disabled {
  display: inline-flex;
}

.bundle-row-head {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}

.bundle-row-name {
  font-weight: 500;
  color: var(--color-text-1);
}

.bundle-chip {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  color: var(--color-text-3);
  background: var(--color-fill-2);
}

.bundle-chip-muted {
  color: var(--color-text-4);
}

.bundle-chip-warn {
  color: #ad6800;
  background: #fff7e6;
}

.bundle-row-name-sub {
  margin-top: 2px;
  font-size: 12px;
  color: var(--color-text-3);
}

.bundle-row-state {
  margin-top: 4px;
  font-size: 12px;
  color: var(--color-text-4);
}

.bundle-empty {
  padding: 10px 12px;
  font-size: 13px;
  color: var(--color-text-3);
}

.bundle-toggle {
  padding: 0;
  margin-bottom: 10px;
  border: none;
  background: none;
  cursor: pointer;
  font-size: 13px;
  color: rgb(var(--primary-6));

  &:hover {
    text-decoration: underline;
  }
}

.plugins-readonly-banner {
  margin-bottom: 12px;
  padding: 8px 12px;
  border-radius: 6px;
  font-size: 13px;
  color: var(--color-text-2);
  background: var(--apple-group-bg);
  border: 1px solid var(--apple-card-border);
}

.plugin-cell-name {
  display: block;
  font-weight: 500;
  color: var(--color-text-1);
}

/* Identity under the title: the module specifier and the version, on one line
   so a row's technical facts stay a unit instead of stacking three deep. */
.plugin-cell-sub {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 3px;
  min-width: 0;
}

.plugin-cell-module {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  color: var(--color-text-4);
}

.plugin-cell-id {
  color: var(--color-text-3);
}

.plugin-phase {
  font-size: 12px;
  color: var(--color-text-3);
}

.plugin-no-version {
  color: var(--color-text-4);
}

.plugins-files-note {
  margin: 10px 0 0;
  font-size: 12px;
  color: var(--color-text-3);
}
</style>

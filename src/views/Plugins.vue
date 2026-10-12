<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import type { BundleItem, ChangeOutcome, PluginItem, PluginView } from '@/api/types'
import DlEmpty from '@/components/DlEmpty.vue'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useLauncherStore()

// --- Selectors: HOME & Profile -----------------------------------------------
// The selectors stay (HOME, Profile); the backend resolves them to the
// instance running that profile (design §4).

const selectedHomeId = ref<string | undefined>(undefined)
const selectedProfile = ref<string | undefined>(undefined)

const selectedHome = computed(() => store.homeById(selectedHomeId.value ?? ''))

const profileOptions = ref<string[]>([])
const profilesLoading = ref(false)

// Initialize / sync selectedHomeId from route.query or store.homes
watch(
  [() => route.query.homeId, () => store.homes],
  ([qHomeId]) => {
    const qStr = typeof qHomeId === 'string' ? qHomeId : undefined
    if (qStr && store.homes.some((h) => h.id === qStr)) {
      selectedHomeId.value = qStr
    } else if (!selectedHomeId.value || !store.homes.some((h) => h.id === selectedHomeId.value)) {
      selectedHomeId.value = store.homes[0]?.id
    }
  },
  { immediate: true },
)

// When selectedHomeId changes, fetch its profile list
watch(
  selectedHomeId,
  async (newHomeId) => {
    profileOptions.value = []
    if (!newHomeId) {
      selectedProfile.value = undefined
      return
    }
    profilesLoading.value = true
    try {
      const list = await api.listProfiles(newHomeId)
      profileOptions.value = list
      const qProfile = typeof route.query.profile === 'string' ? route.query.profile : undefined
      if (qProfile && list.includes(qProfile)) {
        selectedProfile.value = qProfile
      } else if (!selectedProfile.value || !list.includes(selectedProfile.value)) {
        selectedProfile.value = list[0]
      }
    } catch (e) {
      Message.error(String(e))
    } finally {
      profilesLoading.value = false
    }
  },
  { immediate: true },
)

function onSelectHome(id: unknown) {
  const homeId = String(id ?? '')
  selectedHomeId.value = homeId
  void router.replace({
    query: {
      ...route.query,
      homeId,
      profile: undefined,
    },
  })
}

function onSelectProfile(p: unknown) {
  const profile = String(p ?? '')
  selectedProfile.value = profile
  void router.replace({
    query: {
      ...route.query,
      profile,
    },
  })
}

// --- The plugin view of (selectedHomeId, selectedProfile) ---------------------
// One snapshot from the running instance's native plugin manager. `mode`
// decides whether the page manages or only displays; `reason` is shown
// verbatim (the backend localizes it).

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
  { title: t('plugins.pluginName'), slotName: 'pname' },
  { title: t('plugins.pluginEntry'), slotName: 'pentry', width: 320 },
  { title: t('plugins.pluginPhase'), slotName: 'pphase', width: 130 },
  { title: t('plugins.pluginStatus'), slotName: 'pstatus', width: 120 },
])
</script>

<template>
  <div class="dl-page plugins-page">
    <!-- Top Selector Bar -->
    <div class="dl-card scope-selector-bar">
      <div class="selector-row">
        <div class="selector-item">
          <label class="selector-label">{{ t('plugins.selectHome') }}：</label>
          <a-select
            :model-value="selectedHomeId"
            :placeholder="t('plugins.selectHome')"
            class="scope-select"
            @change="onSelectHome"
          >
            <a-option v-for="h in store.homes" :key="h.id" :value="h.id">
              {{ h.name }}
            </a-option>
          </a-select>
        </div>

        <div class="selector-item">
          <label class="selector-label">{{ t('plugins.selectProfile') }}：</label>
          <a-select
            :model-value="selectedProfile"
            :placeholder="t('plugins.selectProfile')"
            :loading="profilesLoading"
            class="scope-select"
            @change="onSelectProfile"
          >
            <a-option v-for="p in profileOptions" :key="p" :value="p">
              {{ p }}
            </a-option>
          </a-select>
        </div>
      </div>
    </div>

    <!-- Patch layers (bundles) card — a layer ABOVE the plugin entries.
         The switch here is a bundle-layer switch; the switch in the entry
         table below is a single-plugin-entry switch. Kept separate so the
         user can tell which layer they are touching. -->
    <div
      v-if="selectedHomeId && selectedProfile && (managed || bundles.length > 0)"
      class="dl-card bundles-card"
    >
      <div class="dl-card-title">
        <div class="title-with-pill">
          <h3>{{ t('plugins.bundlesTitle') }}</h3>
          <span class="layer-tag bundle-layer-tag">{{ t('plugins.bundlesLayerTag') }}</span>
          <span v-if="selectedHome" class="scope-pill">{{ selectedHome.name }} / {{ selectedProfile }}</span>
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
              <span v-if="b.version" class="bundle-row-version tnum">{{ b.version }}</span>
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
                <span v-if="b.version" class="bundle-row-version tnum">{{ b.version }}</span>
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

    <!-- Plugins Table Card -->
    <div v-if="selectedHomeId && selectedProfile" class="dl-card plugins-card">
      <div class="dl-card-title">
        <div class="title-with-pill">
          <h3>{{ t('plugins.pluginsTitle') }}</h3>
          <span class="layer-tag entry-layer-tag">{{ t('plugins.entriesLayerTag') }}</span>
          <span v-if="selectedHome" class="scope-pill">{{ selectedHome.name }} / {{ selectedProfile }}</span>
        </div>
      </div>
      <p class="dl-card-desc">{{ t('plugins.pluginsDesc') }}</p>

      <!-- Read-only banner: no instance running this profile, adopted instance,
           or an unreachable instance. The reason is backend-localized. -->
      <div v-if="!managed && viewReason" class="plugins-readonly-banner">
        {{ viewReason }}
      </div>

      <!-- Batch enable/disable: the same native path as a single toggle. Only
           meaningful while the instance is manageable. -->
      <div v-if="managed && plugins.length > 0" class="plugins-batch-row">
        <div class="batch-btns">
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

      <!-- The table renders whenever the instance gave us entries (managed), or
           the offline read produced a file-sourced list; a pure read-only page
           with nothing to show has only the banner, not a contradictory empty
           table. -->
      <a-table
        v-if="managed || plugins.length > 0"
        :columns="pluginColumns"
        :data="plugins"
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
          <DlEmpty icon="plugin" :description="t('plugins.pluginsEmpty')" />
        </template>
      </a-table>
      <!-- Read-only list read from the profile files: the offline content is a
           best-effort projection, so say how it differs from the instance's. -->
      <p v-if="filesNote" class="plugins-files-note">{{ filesNote }}</p>
    </div>

    <!-- Empty Selection Card -->
    <div v-else class="dl-card">
      <DlEmpty icon="plugin" :description="t('plugins.pluginsPickProfile')" />
    </div>
  </div>
</template>

<style lang="scss" scoped>
.plugins-page {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.scope-selector-bar {
  padding: 14px 18px;

  .selector-row {
    display: flex;
    align-items: center;
    gap: 24px;
    flex-wrap: wrap;

    .selector-item {
      display: flex;
      align-items: center;
      gap: 10px;

      .selector-label {
        font-size: 13px;
        font-weight: 500;
        color: var(--color-text-2);
        white-space: nowrap;
      }

      .scope-select {
        width: 200px;
      }
    }
  }
}

.title-with-pill {
  display: flex;
  align-items: center;
  gap: 10px;

  h3 {
    margin: 0;
  }
}

.plugins-batch-row {
  display: flex;
  align-items: center;
  margin-bottom: 12px;

  .batch-btns {
    display: flex;
    align-items: center;
    gap: 8px;
  }
}

.scope-pill {
  display: inline-flex;
  align-items: center;
  font-size: 12px;
  font-weight: 500;
  padding: 2px 8px;
  border-radius: 6px;
  background: rgb(var(--primary-6) / 10%);
  color: rgb(var(--primary-6));
  border: 1px solid rgb(var(--primary-6) / 20%);
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

.bundle-row-version {
  font-size: 12px;
  color: var(--color-text-3);
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
  font-weight: 500;
  color: var(--color-text-1);
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

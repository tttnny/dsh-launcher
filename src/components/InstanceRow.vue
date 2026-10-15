<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore, type HealthLogItem } from '@/stores/launcher'
import type { DshInstance } from '@/api/types'
import { useAction } from '@/composables/useAction'
import { useCopy } from '@/composables/useCopy'
import { usePluginLink } from '@/composables/usePluginLink'
import { useProfiles } from '@/composables/useProfiles'
import DlIcon from '@/components/DlIcon.vue'
import InstanceAvatar from '@/components/InstanceAvatar.vue'

// One instance on the board.
//
// The row answers exactly two questions — what is it, and is it live — and
// carries the controls that act on those answers: the profile it launches with
// (the one choice made at launch time), its primary action, and — while there
// is a process to drive — that process's own controls (restart / stop /
// plugins). Those three used to live in the card, which meant a click to open
// before a click to act, on the one row the user is actually driving. Facts
// still live in the card the row expands into: expanding the row is for
// reading, not for reaching a control.

const props = defineProps<{
  inst: DshInstance
  /** Set when something handed the board a reason to point at this row. */
  focus?: boolean
}>()
const emit = defineEmits<{ changed: []; duplicate: [id: string] }>()

const router = useRouter()
const { t } = useI18n()
const { copy } = useCopy()
const store = useLauncherStore()
const { profilesByHome, selectionByInstance, loadingByHome, ensureSelection, invalidateHome } =
  useProfiles()
const { canManagePlugins, openPlugins } = usePluginLink()

// --- Derived facts ------------------------------------------------------------

const status = computed(() => store.statusOf(props.inst.id))
const expanded = ref(false)
const rowEl = ref<HTMLElement | null>(null)
const highlighted = ref(false)
function toggleExpanded() {
  expanded.value = !expanded.value
}

/**
 * Comes back expanded and briefly ringed, so "返回实例" lands on the row the
 * user left instead of a board they have to search again.
 */
function reveal() {
  expanded.value = true
  void nextTick(() => {
    rowEl.value?.scrollIntoView({ block: 'center', behavior: 'smooth' })
    highlighted.value = true
    window.setTimeout(() => (highlighted.value = false), 1600)
  })
}

watch(
  () => props.focus,
  (v) => {
    if (v) reveal()
  },
  { immediate: true },
)

/**
 * The whole row is the hit target, but the controls inside it are not: a click
 * or a keypress that belongs to a button, the profile select, or a link is that
 * control's, not the disclosure's. Keyboard matters here because Enter on a
 * focused button fires the button *and* bubbles this far — without the guard,
 * stopping an instance from the row would also expand it.
 */
function onRowControl(target: EventTarget | null) {
  return !!(target as HTMLElement | null)?.closest(
    'button, a, input, textarea, select, .arco-select',
  )
}

function onRowClick(event: MouseEvent) {
  if (onRowControl(event.target)) return
  toggleExpanded()
}

function onRowKeydown(event: KeyboardEvent) {
  if (onRowControl(event.target)) return
  event.preventDefault()
  toggleExpanded()
}
const running = computed(() => status.value.state === 'running')
const starting = computed(() => status.value.state === 'starting')
const versionName = computed(
  () => store.versionById(props.inst.version_id)?.version ?? props.inst.version_id,
)
/**
 * Installing a version names its instance after that version, so the common
 * case has the name and the version saying the same thing twice. The name is
 * the row's title; the chip is only worth showing when it adds something.
 */
const showVersionChip = computed(() => versionName.value !== props.inst.name)
const homeName = computed(() => store.homeById(props.inst.home_id)?.name ?? props.inst.home_id)
const sharedHome = computed(() => store.isHomeShared(props.inst.home_id))
const version = computed(() => store.versionById(props.inst.version_id))
const home = computed(() => store.homeById(props.inst.home_id))
const host = computed(() => store.hostById(props.inst.host_id))
/**
 * Two instances called 「web」 on two machines must not read alike, so a row that
 * belongs to another machine says which one — but only where more than one
 * machine is on screen (the board's 全部 view). Under a single-host filter every
 * row would otherwise repeat the same name the filter already states.
 */
const showHost = computed(() => store.allHosts && !!host.value && !host.value.builtin)
const envEntries = computed(() => Object.entries(props.inst.env_overrides ?? {}))
const portLabel = computed(() =>
  props.inst.port ? String(props.inst.port) : t('instances.portRandom'),
)
/** The in-app self-check entries recorded for this instance. */
const health = computed(() => {
  const mine = store.healthLogs.filter((l) => l.instanceId === props.inst.id)
  return {
    errors: mine.filter((l) => l.level === 'error').length,
    warns: mine.filter((l) => l.level === 'warn').length,
  }
})
const profiles = computed(() => profilesByHome[props.inst.home_id] ?? [])
const profilesLoading = computed(() => !!loadingByHome[props.inst.home_id])

const selectedProfile = computed<string | undefined>({
  get: () => selectionByInstance[props.inst.id],
  set: (v) => {
    selectionByInstance[props.inst.id] = v
  },
})

// --- Profile selection --------------------------------------------------------

watch(
  () => props.inst.home_id,
  () => {
    void ensureSelection(props.inst).then((p) => {
      if (p === undefined) Message.warning(t('home.noProfile'))
    })
  },
  { immediate: true },
)

async function refreshProfiles() {
  invalidateHome(props.inst.home_id)
  await ensureSelection(props.inst)
}

// --- Lifecycle ----------------------------------------------------------------

const startAction = useAction(
  (args: { id: string; profile: string }) => api.startInstance(args.id, args.profile),
  { key: (args) => args.id, success: () => t('home.started') },
)
const stopAction = useAction((id: string) => api.stopInstance(id), {
  key: (id) => id,
  success: () => t('home.stopped'),
})
const restartAction = useAction((id: string) => api.restartInstance(id), {
  key: (id) => id,
  success: () => t('home.started'),
})
const openAction = useAction((id: string) => api.openInstanceWindow(id), { key: (id) => id })

const busy = computed(
  () =>
    !!startAction.busy[props.inst.id] ||
    !!stopAction.busy[props.inst.id] ||
    !!restartAction.busy[props.inst.id],
)

function touchLastUsed() {
  if (store.settings.last_instance_id === props.inst.id) return
  void api.updateSettings({ last_instance_id: props.inst.id }).then((s) => {
    store.settings = s
  })
}

async function reportHealth(profile: string) {
  try {
    const report = await api.checkInstanceHealth(props.inst.id, profile)
    if (!report?.findings?.length) return
    const logs: HealthLogItem[] = report.findings.map((f, idx) => ({
      id: `${Date.now()}-${props.inst.id}-${idx}-${Math.random().toString(36).slice(2, 6)}`,
      timestamp: Date.now(),
      instanceId: props.inst.id,
      instanceName: props.inst.name,
      profile,
      level: f.level,
      code: f.code,
      message: f.message,
    }))
    store.addHealthLogs(logs)
  } catch {
    // Advisory only
  }
}

async function onStart() {
  const profile = selectedProfile.value
  if (!profile || busy.value) return
  const ok = await startAction.run({ id: props.inst.id, profile })
  if (ok === undefined) return
  touchLastUsed()
  void reportHealth(profile)
}

async function onOpen() {
  const ok = await openAction.run(props.inst.id)
  if (ok === undefined) return
  touchLastUsed()
}

async function onStop() {
  await stopAction.run(props.inst.id)
}

async function onRestart() {
  const ok = await restartAction.run(props.inst.id)
  if (ok === undefined) return
  const profile = status.value.profile ?? selectedProfile.value
  if (profile) void reportHealth(profile)
}

// --- Utilities ----------------------------------------------------------------

const dirAction = useAction(() => api.openInstanceDirectory(props.inst.id), {
  success: (path) => t('instanceEdit.dirOpened', { path }),
})
const logAction = useAction(() => api.openInstanceLog(props.inst.id), {
  success: (path) => t('instanceEdit.logOpened', { path }),
})
const terminalAction = useAction(() => api.openInstanceTerminal(props.inst.id), {
  success: (label) => t('home.terminalOpened', { label }),
})

function openDetail() {
  void router.push({ name: 'instance-detail', params: { id: props.inst.id } }).catch(() => undefined)
}

onMounted(() => {
  if (status.value.state === 'running') {
    void ensureSelection(props.inst).then((p) => {
      if (p) void reportHealth(p)
    })
  }
})
</script>

<template>
  <div
    ref="rowEl"
    class="inst-row"
    :class="{
      'is-live': running,
      'is-starting': starting,
      'is-expanded': expanded,
      'is-focus': highlighted,
    }"
  >
    <span class="inst-rail" aria-hidden="true" />

    <div
      class="inst-line"
      tabindex="0"
      @click="onRowClick"
      @keydown.enter.space="onRowKeydown"
    >

    <InstanceAvatar :id="inst.id" :icon="inst.icon" :size="36" />

    <div class="inst-main">
      <div class="inst-title-line">
        <button
          class="inst-name"
          :title="inst.name"
          :aria-expanded="expanded"
          @click.stop="toggleExpanded"
        >
          {{ inst.name }}
        </button>
        <span v-if="sharedHome" class="dl-tag dl-tag--warn">{{ t('home.sharedHome') }}</span>
      </div>
      <div class="inst-meta">
        <template v-if="showHost">
          <span class="dl-chip" :title="host?.ssh_target || host?.display_name">
            {{ host?.display_name }}
          </span>
          <span class="inst-meta-sep">·</span>
        </template>
        <template v-if="showVersionChip">
          <span class="dl-chip" :title="versionName">{{ versionName }}</span>
          <span class="inst-meta-sep">·</span>
        </template>
        <span class="inst-meta-home" :title="homeName">{{ homeName }}</span>
      </div>

      <!-- The live URL is the only thing on the row that gets the run colour:
           it is what "running" actually means and what the user clicks. -->
      <div v-if="running && status.url" class="inst-url">
        <button class="inst-url-link" @click="onOpen">
          <DlIcon name="link" :size="12" />
          <span class="dl-mono">{{ status.url }}</span>
        </button>
        <button class="dl-icon-btn inst-url-copy" :title="t('common.copy')" @click="copy(status.url!)">
          <DlIcon name="copy" :size="12" />
        </button>
      </div>
    </div>

    <!-- The row's controls, stacked: what the instance is running on top, and —
         only while there is a process to drive — that process's own controls
         underneath, where they are one click away instead of one click and an
         expansion away. -->
    <div class="inst-tail">
      <div class="inst-tail-line">
        <div class="inst-profile">
          <a-select
            v-if="!running && !starting"
            v-model="selectedProfile"
            :placeholder="t('home.selectProfile')"
            :loading="profilesLoading"
            size="small"
            :allow-clear="false"
            style="width: 132px"
            @change="touchLastUsed"
          >
            <a-option v-for="p in profiles" :key="p" :value="p">{{ p }}</a-option>
          </a-select>
          <span v-else class="dl-chip" :title="status.profile ?? selectedProfile">
            {{ status.profile ?? selectedProfile ?? t('home.selectProfile') }}
          </span>
        </div>

        <span class="dl-status inst-status" :class="`is-${status.state}`">
          {{ t(`home.status.${status.state}`) }}
        </span>

        <div class="inst-actions">
          <button
            v-if="running"
            class="dl-btn dl-btn--live inst-primary"
            :disabled="!!openAction.busy[inst.id]"
            @click="onOpen"
          >
            <DlIcon name="external" :size="13" />
            {{ t('home.openBrowser') }}
          </button>
          <button
            v-else-if="starting"
            class="dl-btn dl-btn--quiet inst-primary"
            disabled
          >
            <span class="dl-spinner" />
            {{ t('home.status.starting') }}
          </button>
          <button
            v-else
            class="dl-btn dl-btn--primary inst-primary"
            :disabled="!selectedProfile || busy || !store.versionById(inst.version_id)"
            @click="onStart"
          >
            <DlIcon name="play" :size="13" />
            {{ t('home.start') }}
          </button>
        </div>
      </div>

      <!-- Acts on the process, so it only exists while there is one. -->
      <div v-if="running || starting" class="inst-tail-line inst-proc">
        <button class="dl-btn dl-btn--quiet" :disabled="busy" @click="onRestart">
          <DlIcon name="restart" :size="13" />
          {{ t('home.restart') }}
        </button>
        <button class="dl-btn dl-btn--quiet inst-stop" :disabled="busy" @click="onStop">
          <DlIcon name="stop" :size="13" />
          {{ t('home.stop') }}
        </button>
        <button
          v-if="canManagePlugins(inst.id)"
          class="dl-btn dl-btn--quiet"
          @click="openPlugins(inst.id, 'instances')"
        >
          <DlIcon name="plugin" :size="13" />
          {{ t('profiles.managePlugins') }}
        </button>
      </div>
    </div>

    <!-- The disclosure mark, at the far edge where it reads as "this row opens"
         rather than as another button next to the name. -->
    <DlIcon name="chevron-down" :size="15" class="inst-chev" :class="{ 'is-open': expanded }" />
    </div>

    <!-- Expanded: the facts that otherwise cost a trip to the settings page,
         and the actions that have no business being in a hover menu. -->
    <div v-if="expanded" class="inst-facts">
      <div class="facts-grid">
      <div class="fact">
        <span class="fact-label">{{ t('instanceEdit.version') }}</span>
        <span class="fact-value dl-mono" :title="version?.dir">{{ versionName }}</span>
      </div>
      <div class="fact">
        <span class="fact-label">{{ t('instanceEdit.home') }}</span>
        <span class="fact-value" :title="home?.path">
          {{ homeName }}
          <span class="fact-sub dl-mono">{{ home?.path }}</span>
        </span>
      </div>
      <div class="fact">
        <span class="fact-label">{{ t('instanceEdit.defaultProfile') }}</span>
        <span class="fact-value">{{ inst.default_profile || t('home.profileUnset') }}</span>
      </div>
      <div class="fact">
        <span class="fact-label">{{ t('instanceEdit.port') }}</span>
        <span class="fact-value dl-mono">{{ portLabel }}</span>
      </div>
      <div class="fact">
        <span class="fact-label">
          {{ t('instanceEdit.env') }}
          <span v-if="envEntries.length" class="fact-count">{{ envEntries.length }}</span>
        </span>
        <span class="fact-value fact-chips">
          <template v-if="envEntries.length">
            <span
              v-for="[key, value] in envEntries"
              :key="key"
              class="fact-chip dl-mono"
              :title="`${key}=${value}`"
            >
              {{ key }}
            </span>
          </template>
          <span v-else class="fact-sub">{{ t('instances.envNone') }}</span>
        </span>
      </div>
      <div class="fact">
        <span class="fact-label">{{ t('home.healthLogs') }}</span>
        <span class="fact-value">
          <span v-if="health.errors > 0" class="fact-bad">
            {{ t('instances.healthErrors', { count: health.errors }) }}
          </span>
          <span v-else-if="health.warns > 0" class="fact-warn">
            {{ t('instances.healthWarns', { count: health.warns }) }}
          </span>
          <span v-else class="fact-sub">{{ t('instances.healthOk') }}</span>
        </span>
      </div>
      </div>

      <!-- The action bar is grouped by what an action acts on, and the groups
           are laid out on their own lines: the machine first, then the binding.
           The process group is not here — a live process is driven from the row
           above, which is already reporting it. One wrapping row used to strand
           a single button on a line of its own. -->
      <div class="facts-actions">
        <div class="facts-line">
          <button class="dl-btn dl-btn--quiet" @click="terminalAction.run()">
            <DlIcon name="terminal" :size="13" />
            {{ t('home.openTerminal') }}
          </button>
          <button class="dl-btn dl-btn--quiet" @click="dirAction.run()">
            <DlIcon name="folder" :size="13" />
            {{ t('instanceEdit.openDir') }}
          </button>
          <button class="dl-btn dl-btn--quiet" @click="logAction.run()">
            <DlIcon name="log" :size="13" />
            {{ t('instanceEdit.viewLog') }}
          </button>
        </div>

        <!-- The binding group, on its own line: everything here acts on the
             binding itself, never on this process or this machine. The
             settings page belongs with them — it is the same three facts,
             editable. A copy is a new binding of the same version + HOME, so
             it is the same sheet the New instance button opens, pre-filled. -->
        <div class="facts-line">
          <button class="dl-btn dl-btn--quiet" @click="openDetail">
            <DlIcon name="settings" :size="13" />
            {{ t('instanceEdit.titleEdit') }}
          </button>
          <button class="dl-btn dl-btn--quiet" @click="emit('duplicate', inst.id)">
            <DlIcon name="copy" :size="13" />
            {{ t('instances.duplicate') }}
          </button>
          <button
            v-if="!running && !starting"
            class="dl-btn dl-btn--quiet"
            :disabled="profilesLoading"
            @click="refreshProfiles"
          >
            <DlIcon name="refresh" :size="13" />
            {{ t('home.reloadProfiles') }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.inst-row {
  position: relative;

  // Rows are only separated by a hairline; there is no card per instance.
  & + .inst-row {
    border-top: 1px solid var(--dl-line);
  }

  // Arriving back from the plugin manager: the row says "here", once.
  &.is-focus .inst-line {
    animation: inst-focus 1.6s var(--dl-ease);
  }

  &.is-focus .inst-rail {
    background: var(--dl-signal);
    opacity: 1;
  }
}

@keyframes inst-focus {
  from {
    background-color: var(--dl-signal-soft);
  }
}
// The row proper; the facts panel below it is a sibling so a row can grow
// downward without turning into a nested flex mess.
.inst-line {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 13px 16px 13px 18px;
  cursor: pointer;
  transition: background-color var(--dl-fast) var(--dl-ease);

  &:hover {
    background-color: var(--dl-surface-2);
  }

  // Running instances are marked by a rail + a faint wash: state is visible
  // from across the room without reading a single label.
  &.is-live {
    background: linear-gradient(90deg, var(--dl-run-soft), transparent 46%);

    &:hover {
      background: linear-gradient(90deg, var(--dl-run-soft), var(--dl-surface-2) 46%);
    }

    .inst-rail {
      background: var(--dl-run);
      opacity: 1;
    }
  }

  &.is-starting .inst-rail {
    background: var(--dl-warn);
    opacity: 1;
  }

  &:focus-visible {
    outline: 2px solid var(--dl-signal-line);
    outline-offset: -3px;
  }
}

.inst-rail {
  position: absolute;
  left: 0;
  top: 10px;
  bottom: 10px;
  width: 3px;
  border-radius: 0 2px 2px 0;
  background: transparent;
  opacity: 0;
  transition: opacity var(--dl-slow) var(--dl-ease);
}

.inst-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.inst-title-line {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.inst-name {
  padding: 0;
  border: none;
  background: none;
  color: var(--dl-ink);
  font-family: inherit;
  font-size: var(--dl-text-lg);
  font-weight: 600;
  letter-spacing: -0.018em;
  cursor: pointer;
  min-width: 0;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;

  &:hover {
    color: var(--dl-signal);
  }
}

// The disclosure mark: at the row's outer edge, quiet until the row is under
// the cursor or open, so it reads as part of the row rather than a control.
.inst-chev {
  flex-shrink: 0;
  margin-left: 2px;
  margin-right: -2px;
  color: var(--dl-ink-4);
  transition:
    transform var(--dl-slow) var(--dl-ease),
    color var(--dl-fast) var(--dl-ease);

  &.is-open {
    transform: rotate(180deg);
  }
}

.inst-line:hover .inst-chev,
.inst-row.is-expanded .inst-chev {
  color: var(--dl-ink-2);
}

// The expanded fact sheet: a quiet second surface inside the row.
.inst-facts {
  padding: 12px 16px 16px 82px;
  background: var(--dl-surface-2);
  border-top: 1px solid var(--dl-line);
  animation: facts-in var(--dl-fast) var(--dl-ease);
}

.facts-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
  gap: 10px 24px;
}

.facts-actions {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
  margin-top: 14px;
}

// One line per group, each wrapping inside itself: a group never breaks in the
// middle of another, so no button is ever stranded on a line of its own.
.facts-line {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
}

@keyframes facts-in {
  from {
    opacity: 0;
    transform: translateY(-3px);
  }
}

.fact {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.fact-label {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-4);
}

.fact-count {
  padding: 0 4px;
  border-radius: 4px;
  background: var(--dl-surface-3);
  color: var(--dl-ink-3);
}

.fact-value {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.fact-sub {
  display: block;
  color: var(--dl-ink-4);
  overflow: hidden;
  text-overflow: ellipsis;
}

.fact-chips {
  display: flex;
  gap: 6px;
  overflow: hidden;
}

.fact-chip {
  padding: 0 6px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface);
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-2);
}

.fact-bad {
  color: var(--dl-danger);
}

.fact-warn {
  color: var(--dl-warn);
}


.inst-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.inst-meta-sep {
  color: var(--dl-ink-4);
}

.inst-meta-home {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.inst-url {
  display: flex;
  align-items: center;
  gap: 2px;
  margin-top: 3px;

  .inst-url-link {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 340px;
    padding: 1px 0;
    border: none;
    background: none;
    color: var(--dl-run);
    font-size: var(--dl-text-sm);
    font-weight: 550;
    cursor: pointer;
    overflow: hidden;

    span {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

    &:hover {
      text-decoration: underline;
    }
  }

  .inst-url-copy {
    width: 22px;
    height: 22px;
  }
}

// The row's control block: a column, so the process controls can sit on their
// own line under the primary action without widening the row. Right-aligned on
// both lines — the row's controls end at one edge, whatever they are.
.inst-tail {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 8px;
  flex-shrink: 0;
}

.inst-tail-line {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
}

// Grouped tighter than the facts beside them: these three act on one thing.
.inst-proc {
  gap: 4px;
}

// Stopping is the one control on the row that takes something away; it says so
// on hover instead of wearing red all the time.
.inst-stop:hover:not(:disabled) {
  border-color: var(--dl-danger);
  color: var(--dl-danger);
}

.inst-profile {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}

.inst-status {
  width: 62px;
  flex-shrink: 0;
}

.inst-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.inst-primary {
  min-width: 84px;
}

.dl-spinner {
  width: 11px;
  height: 11px;
  border: 2px solid currentColor;
  border-right-color: transparent;
  border-radius: 50%;
  animation: dl-spin 0.7s linear infinite;
}

@keyframes dl-spin {
  to {
    transform: rotate(360deg);
  }
}

</style>

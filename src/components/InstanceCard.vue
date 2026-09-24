<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import type { HealthLogItem } from '@/stores/launcher'
import type { DshInstance } from '@/api/types'
import { useAction } from '@/composables/useAction'
import { useProfiles } from '@/composables/useProfiles'
import launcherDefaultIcon from '@/assets/launcher-icon.png'

// InstanceCard: one instance as a deep module. It owns everything per-card —
// profile list + selection (via useProfiles), icon, busy flags, the start /
// stop / restart lifecycle actions, and its own health report — so the parent
// keeps only the filter and the grid.

const props = defineProps<{ inst: DshInstance }>()

const router = useRouter()
const { t } = useI18n()
const store = useLauncherStore()
const { profilesByHome, selectionByInstance, loadingByHome, loadForHome, ensureSelection, invalidateHome } =
  useProfiles()

// --- Derived card facts ------------------------------------------------------

const status = computed(() => store.statusOf(props.inst.id))
const versionName = computed(() => store.versionById(props.inst.version_id)?.version ?? props.inst.version_id)
const homeName = computed(() => store.homeById(props.inst.home_id)?.name ?? props.inst.home_id)
const sharedHome = computed(() => store.isHomeShared(props.inst.home_id))
const profiles = computed(() => profilesByHome[props.inst.home_id] ?? [])
const profilesLoading = computed(() => !!loadingByHome[props.inst.home_id])
const selectedProfile = computed<string | undefined>({
  get: () => selectionByInstance[props.inst.id],
  set: (v) => {
    selectionByInstance[props.inst.id] = v
  },
})

// --- Instance icon ------------------------------------------------------------

const icon = ref<string | null>(null)

async function loadIcon() {
  if (!props.inst.icon) {
    icon.value = null
    return
  }
  try {
    icon.value = await api.readInstanceIcon(props.inst.id)
  } catch {
    icon.value = null
  }
}

watch(() => props.inst.icon, loadIcon, { immediate: true })

// --- Profile selection ---------------------------------------------------------

async function refreshProfiles() {
  invalidateHome(props.inst.home_id)
  await ensureSelection(props.inst)
}

watch(
  () => props.inst.home_id,
  () => {
    void ensureSelection(props.inst).then((p) => {
      if (p === undefined) Message.warning(t('home.noProfile'))
    })
  },
  { immediate: true },
)

// --- Lifecycle actions ---------------------------------------------------------

const restartBusy = useAction((id: string) => api.restartInstance(id), {
  key: (id) => id,
  success: () => t('home.started'),
})
const restarting = computed(() => restartBusy.busy[props.inst.id])

const canStart = computed(() => {
  const st = status.value.state
  return (
    !!selectedProfile.value &&
    st !== 'starting' &&
    st !== 'running' &&
    !restarting.value &&
    !!store.versionById(props.inst.version_id)
  )
})

async function reportHealth(profile: string) {
  try {
    const report = await api.checkInstanceHealth(props.inst.id, profile)
    if (report?.findings?.length) {
      const instName = props.inst.name
      const newLogs: HealthLogItem[] = report.findings.map((f, idx) => ({
        id: `${Date.now()}-${props.inst.id}-${idx}-${Math.random().toString(36).slice(2, 6)}`,
        timestamp: Date.now(),
        instanceId: props.inst.id,
        instanceName: instName,
        profile,
        level: f.level,
        code: f.code,
        message: f.message,
      }))
      store.addHealthLogs(newLogs)
    }
  } catch {
    // Advisory only
  }
}

onMounted(() => {
  if (status.value.state === 'running') {
    void ensureSelection(props.inst).then((p) => {
      if (p) void reportHealth(p)
    })
  }
})

function touchLastUsed() {
  if (store.settings.last_instance_id === props.inst.id) return
  void api.updateSettings({ last_instance_id: props.inst.id }).then((s) => {
    store.settings = s
  })
}

const startAction = useAction((args: { id: string; profile: string }) => api.startInstance(args.id, args.profile), {
  key: (args) => args.id,
  success: () => t('home.started'),
})
const stopAction = useAction((id: string) => api.stopInstance(id), {
  key: (id) => id,
  success: () => t('home.stopped'),
})
const openAction = useAction((id: string) => api.openInstanceWindow(id), {
  key: (id) => id,
})

async function onStart() {
  const profile = selectedProfile.value
  if (!profile || restarting.value) return
  const ok = await startAction.run({ id: props.inst.id, profile })
  if (ok === undefined) return
  touchLastUsed()
  void reportHealth(profile)
}

async function onStop() {
  await stopAction.run(props.inst.id)
}

async function onRestart() {
  const ok = await restartBusy.run(props.inst.id)
  if (ok === undefined) return
  const profile = status.value.profile ?? selectedProfile.value
  if (profile) void reportHealth(profile)
}

async function onOpenBrowser() {
  const ok = await openAction.run(props.inst.id)
  if (ok === undefined) return
  touchLastUsed()
}

// --- Overflow actions ------------------------------------------------------------

function goSettings() {
  void router.push({ name: 'instance-edit', params: { id: props.inst.id } }).catch(() => undefined)
}

const dirAction = useAction(() => api.openInstanceDirectory(props.inst.id), {
  success: (path) => t('instanceEdit.dirOpened', { path }),
})
const logAction = useAction(() => api.openInstanceLog(props.inst.id), {
  success: (path) => t('instanceEdit.logOpened', { path }),
})
const terminalAction = useAction(() => api.openInstanceTerminal(props.inst.id), {
  success: (label) => t('home.terminalOpened', { label }),
})
const homeAction = useAction(
  async (newHomeId: string) => {
    const updated: DshInstance = { ...props.inst, home_id: newHomeId }
    await api.updateInstance(updated)
    await store.refreshInstances()
    return true
  },
  { success: () => t('instanceEdit.saved') },
)

async function onChangeHome(newHomeId: string) {
  if (!newHomeId || newHomeId === props.inst.home_id) return
  const st = status.value.state
  if (st === 'running' || st === 'starting') {
    Message.warning(t('home.runningCannotChangeHome') || '实例运行中，请先停止实例再切换 HOME')
    return
  }
  const ok = await homeAction.run(newHomeId)
  if (ok === undefined) return
  delete selectionByInstance[props.inst.id]
  await refreshProfiles()
}

function copyUrl(url: string) {
  navigator.clipboard?.writeText(url)
  Message.success(t('common.copied'))
}
</script>

<template>
  <div class="dl-card instance-card">
    <!-- Card Header -->
    <div class="card-head">
      <div class="instance-avatar">
        <img :src="icon ?? launcherDefaultIcon" alt="" />
      </div>
      <div class="card-title-block">
        <div class="card-name" :title="inst.name">{{ inst.name }}</div>
        <div class="card-meta tnum">{{ versionName }} · {{ homeName }}</div>
      </div>
      <!-- Apple Status Indicator -->
      <div class="status-indicator-wrap">
        <span :class="['apple-status-dot', status.state]">
          {{ t(`home.status.${status.state}`) }}
        </span>
      </div>
    </div>

    <!-- Shared Home Indicator -->
    <div v-if="sharedHome" class="card-shared-badge">
      <a-tooltip :content="t('home.sharedHomeWarning')">
        <span class="shared-chip">
          <svg viewBox="0 0 12 12" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.6">
            <circle cx="6" cy="6" r="4.5" />
            <line x1="6" y1="3.5" x2="6" y2="6.5" />
            <circle cx="6" cy="8.5" r="0.5" fill="currentColor" />
          </svg>
          {{ t('home.sharedHome') }}
        </span>
      </a-tooltip>
    </div>

    <!-- Home Selection Row -->
    <div class="card-profile-row">
      <span class="field-label">HOME</span>
      <div class="profile-select-capsule">
        <a-select
          :model-value="inst.home_id"
          size="small"
          class="card-profile-select"
          :disabled="status.state === 'running' || status.state === 'starting' || homeAction.busy['*']"
          :loading="homeAction.busy['*']"
          @change="(val) => onChangeHome(String(val))"
        >
          <a-option v-for="h in store.homes" :key="h.id" :value="h.id">{{ h.name }}</a-option>
        </a-select>
      </div>
    </div>

    <!-- Profile Selection Row -->
    <div class="card-profile-row">
      <span class="field-label">{{ t('home.profile') }}</span>
      <div class="profile-select-capsule">
        <a-select
          v-model="selectedProfile"
          :placeholder="t('home.selectProfile')"
          :loading="profilesLoading"
          size="small"
          class="card-profile-select"
          allow-clear
          @change="touchLastUsed()"
        >
          <a-option v-for="p in profiles" :key="p" :value="p">{{ p }}</a-option>
        </a-select>
        <button
          class="profile-refresh-btn"
          :title="t('common.refresh')"
          :disabled="profilesLoading"
          @click="refreshProfiles"
        >
          <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
            <path d="M13.5 8A5.5 5.5 0 1 1 12 4.1L14 2" />
            <polyline points="14 5.5 14 2 10.5 2" />
          </svg>
        </button>
      </div>
    </div>

    <!-- Running URL Banner -->
    <div v-if="status.state === 'running' && status.url" class="card-url-banner">
      <button class="url-chip" @click="onOpenBrowser">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
          <path d="M7 9l5-5m0 0H8m4 0v4" />
          <rect x="3" y="6" width="7" height="7" rx="1.5" />
        </svg>
        <span class="url-text tnum">{{ status.url }}</span>
      </button>
      <button class="url-copy-btn" :title="t('common.copy')" @click="copyUrl(status.url!)">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
          <rect x="5" y="5" width="8" height="8" rx="1.5" />
          <path d="M3 11V3a1 1 0 0 1 1-1h8" />
        </svg>
      </button>
    </div>

    <!-- Primary Action Buttons -->
    <div class="card-main-actions">
      <template v-if="restarting">
        <button class="action-btn secondary-action is-busy" disabled>
          <span class="mini-spinner" />
          <span>{{ t('home.restart') }}</span>
        </button>
      </template>

      <template v-else-if="status.state === 'running'">
        <button class="action-btn primary-action active-open" @click="onOpenBrowser">
          <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <polygon points="5 3 13 8 5 13 5 3" />
          </svg>
          <span>{{ t('home.openBrowser') }}</span>
        </button>
        <button class="action-btn secondary-action" @click="onRestart">
          <span>{{ t('home.restart') }}</span>
        </button>
        <button class="action-btn danger-action" @click="onStop">
          <span>{{ t('home.stop') }}</span>
        </button>
      </template>

      <template v-else-if="status.state === 'starting'">
        <button class="action-btn secondary-action is-busy" disabled>
          <span class="mini-spinner" />
          <span>{{ t('home.status.starting') }}</span>
        </button>
        <button class="action-btn danger-action" @click="onStop">
          <span>{{ t('home.stop') }}</span>
        </button>
      </template>

      <template v-else>
        <button
          class="action-btn primary-action"
          :disabled="!canStart"
          @click="onStart"
        >
          <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <polygon points="5 3 13 8 5 13 5 3" />
          </svg>
          <span>{{ t('home.start') }}</span>
        </button>
      </template>
    </div>

    <!-- Card Bottom Utility Bar -->
    <div class="card-bottom-bar">
      <button class="mac-micro-btn" :title="t('home.openTerminal')" :disabled="terminalAction.busy['*']" @click="terminalAction.run()">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
          <polyline points="4 5 7 8 4 11" />
          <line x1="8" y1="12" x2="12" y2="12" />
        </svg>
        <span>{{ t('home.terminal') }}</span>
      </button>

      <button class="mac-micro-btn" :title="t('instanceEdit.openDir')" :disabled="dirAction.busy['*']" @click="dirAction.run()">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
          <path d="M2 4a1.5 1.5 0 0 1 1.5-1.5h3l2 2H13a1.5 1.5 0 0 1 1.5 1.5v6a1.5 1.5 0 0 1-1.5 1.5H3.5A1.5 1.5 0 0 1 2 12V4z" />
        </svg>
        <span>{{ t('instanceEdit.openDir') }}</span>
      </button>

      <button class="mac-micro-btn" :title="t('instanceEdit.viewLog')" :disabled="logAction.busy['*']" @click="logAction.run()">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
          <rect x="3" y="2" width="10" height="12" rx="1.5" />
          <line x1="6" y1="6" x2="10" y2="6" />
          <line x1="6" y1="9" x2="10" y2="9" />
        </svg>
        <span>{{ t('instanceEdit.viewLog') }}</span>
      </button>

      <span class="flex-spacer" />

      <button class="mac-micro-btn icon-only" :title="t('home.cardSettings')" @click="goSettings">
        <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
          <circle cx="8" cy="8" r="2.5" />
          <path d="M8 1.5v1.2M8 13.3v1.2M1.5 8h1.2M13.3 8h1.2M3.4 3.4l.8.8M11.8 11.8l.8.8M3.4 12.6l.8-.8M11.8 4.2l.8-.8" />
        </svg>
      </button>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.instance-card {
  margin-top: 0 !important;
  height: 100%;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 18px;
  border-radius: var(--dl-card-radius);

  &:hover {
    transform: translateY(-2px);
    box-shadow: var(--apple-card-hover-shadow);
  }
}

.card-head {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}

.instance-avatar {
  width: 44px;
  height: 44px;
  border-radius: 11px;
  background: linear-gradient(135deg, #165dff, #722ed1);
  box-shadow: 0 2px 8px rgb(0 0 0 / 12%);
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  flex-shrink: 0;

  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
}

.card-title-block {
  flex: 1;
  min-width: 0;
}

.card-name {
  font-size: 15px;
  font-weight: 600;
  letter-spacing: -0.015em;
  color: var(--color-text-1);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.card-meta {
  font-size: 12px;
  color: var(--color-text-3);
  margin-top: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.status-indicator-wrap {
  flex-shrink: 0;
}

.card-shared-badge {
  .shared-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 7px;
    font-size: 11px;
    border-radius: 6px;
    background: rgb(var(--orange-6) / 12%);
    color: rgb(var(--orange-6));
    font-weight: 500;
  }
}

// Profile Row
.card-profile-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;

  .field-label {
    color: var(--color-text-3);
    font-weight: 500;
    flex-shrink: 0;
  }

  .profile-select-capsule {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .card-profile-select {
    flex: 1;
  }

  .profile-refresh-btn {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: 1px solid var(--apple-card-border);
    background: var(--apple-group-bg);
    color: var(--color-text-3);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: all 0.15s ease;

    &:hover:not(:disabled) {
      color: var(--color-text-1);
    }
  }
}

// URL Banner
.card-url-banner {
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--apple-group-bg);
  border-radius: 8px;
  padding: 4px 8px;
  font-size: 12px;

  .url-chip {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 6px;
    border: none;
    background: transparent;
    color: rgb(var(--primary-6));
    font-weight: 500;
    cursor: pointer;
    text-align: left;
    overflow: hidden;

    .url-text {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

    &:hover {
      text-decoration: underline;
    }
  }

  .url-copy-btn {
    border: none;
    background: transparent;
    color: var(--color-text-3);
    cursor: pointer;
    padding: 3px;
    border-radius: 4px;

    &:hover {
      color: var(--color-text-1);
      background: var(--apple-group-bg);
    }
  }
}

// Card Actions
.card-main-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 4px;

  .action-btn {
    flex: 1;
    height: 32px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 500;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    border: none;
    cursor: pointer;
    transition: all 0.16s ease;

    &.primary-action {
      background: rgb(var(--primary-6));
      color: #fff;
      box-shadow: 0 1px 3px rgb(var(--primary-6) / 25%);

      &:hover:not(:disabled) {
        filter: brightness(1.08);
      }

      &.active-open {
        background: rgb(var(--green-6));
        box-shadow: 0 1px 3px rgb(var(--green-6) / 25%);
      }

      &:disabled {
        opacity: 0.45;
        cursor: not-allowed;
      }
    }

    &.secondary-action {
      background: var(--apple-group-bg);
      color: var(--color-text-1);
      border: 1px solid var(--apple-card-border);

      &:hover:not(:disabled) {
        filter: brightness(0.96);
      }
    }

    &.danger-action {
      flex: 0 0 68px;
      background: rgb(var(--red-6) / 12%);
      color: rgb(var(--red-6));

      &:hover {
        background: rgb(var(--red-6) / 20%);
      }
    }

    &:active:not(:disabled) {
      transform: scale(var(--apple-active-scale));
    }
  }
}

.mini-spinner {
  width: 12px;
  height: 12px;
  border: 2px solid currentColor;
  border-right-color: transparent;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

// Bottom Bar
.card-bottom-bar {
  margin-top: auto;
  display: flex;
  align-items: center;
  gap: 6px;
  padding-top: 10px;
  border-top: 1px solid var(--apple-separator);

  .flex-spacer {
    flex: 1;
  }
}

.mac-micro-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px;
  font-size: 11.5px;
  border-radius: 6px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--color-text-3);
  cursor: pointer;
  transition: all 0.15s ease;

  &:hover:not(:disabled) {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
    border-color: var(--apple-card-border);
  }

  &.icon-only {
    padding: 5px;
  }

  &:active:not(:disabled) {
    transform: scale(var(--apple-active-scale));
  }
}
</style>

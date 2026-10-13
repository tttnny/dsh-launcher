<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { useAction } from '@/composables/useAction'
import { usePluginLink } from '@/composables/usePluginLink'
import { useLauncherStore } from '@/stores/launcher'
import type { DshInstance } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'
import InstanceAvatar from '@/components/InstanceAvatar.vue'

// Everything about one instance. The fields are the binding (name, version,
// HOME) and the launch options (default profile, port), then the environment
// overrides; the danger zone removes the instance and nothing else — the
// version and the HOME are shared resources and outlive it.

const route = useRoute()
const router = useRouter()
const { t } = useI18n()
const store = useLauncherStore()
const { canManagePlugins, openPlugins } = usePluginLink()

const id = computed(() => (route.params.id as string | undefined) ?? '')

const name = ref('')
const versionId = ref<string | undefined>(undefined)
const homeId = ref<string | undefined>(undefined)
const defaultProfile = ref<string | undefined>(undefined)
const profiles = ref<string[]>([])
const portInput = ref('')
const loaded = ref(false)

interface EnvRow {
  key: string
  value: string
}
const envRows = ref<EnvRow[]>([])

const ENV_KEY_RE = /^[A-Za-z_][A-Za-z0-9_]*$/
const RESERVED_KEYS = new Set(['DSH_HOME'])

function envKeyError(row: EnvRow): string | null {
  if (!row.key) return null
  if (RESERVED_KEYS.has(row.key)) return t('instanceEdit.envKeyReserved')
  if (!ENV_KEY_RE.test(row.key)) return t('instanceEdit.envKeyInvalid')
  return null
}

const envValid = computed(() => envRows.value.every((r) => !envKeyError(r)))

onMounted(async () => {
  const inst =
    store.instanceById(id.value) ??
    (await api.listInstances()).find((i) => i.id === id.value)
  if (!inst) {
    Message.error(t('instanceEdit.notFound'))
    void router.replace({ name: 'instances' })
    return
  }
  fillForm(inst)
  loaded.value = true
  await nextTick()
  baseline.value = formSnapshot()
})

const current = computed(() => store.instanceById(id.value))
const status = computed(() => store.statusOf(id.value))
const running = computed(() => status.value.state === 'running' || status.value.state === 'starting')
const homeLabel = computed(() => store.homeById(homeId.value ?? '')?.name ?? '—')

/**
 * The form's identity, used to tell "nothing to save" from "the user changed
 * something". Port is not part of it: the port field has its own apply button
 * and is not written by Save.
 */
function formSnapshot(): string {
  return JSON.stringify([
    name.value.trim(),
    versionId.value ?? '',
    homeId.value ?? '',
    defaultProfile.value ?? '',
    envRows.value
      .filter((r) => r.key.trim() || r.value)
      .map((r) => [r.key.trim(), r.value])
      .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)),
  ])
}

const baseline = ref('')
/** Loads the instance's saved values into the form. */
function fillForm(inst: DshInstance) {
  name.value = inst.name
  versionId.value = inst.version_id
  homeId.value = inst.home_id
  defaultProfile.value = inst.default_profile ?? undefined
  portInput.value = inst.port ? String(inst.port) : ''
  envRows.value = Object.entries(inst.env_overrides).map(([key, value]) => ({ key, value }))
}

// --- Profiles of the selected HOME --------------------------------------------

watch(homeId, async (v) => {
  profiles.value = []
  if (!v) return
  try {
    profiles.value = await api.listProfiles(v)
  } catch (e) {
    Message.error(String(e))
  }
})

/**
 * Switching the HOME invalidates the chosen default profile. This lives on the
 * control, not in the watcher above: a watcher cannot tell "the user picked
 * another HOME" from "the form is loading", and clearing on load wiped the
 * instance's default profile every time the page opened (so saving an untouched
 * form silently dropped it).
 */
function onHomePicked() {
  defaultProfile.value = undefined
}

// --- Save ---------------------------------------------------------------------

const formValid = computed(
  () => name.value.trim().length > 0 && !!versionId.value && !!homeId.value && envValid.value,
)

const saveAction = useAction(
  async () => {
    const inst = current.value
    if (!inst) return undefined
    const envOverrides: Record<string, string> = {}
    for (const row of envRows.value) if (row.key) envOverrides[row.key] = row.value
    const updated: DshInstance = {
      ...inst,
      name: name.value.trim(),
      version_id: versionId.value!,
      home_id: homeId.value!,
      env_overrides: envOverrides,
      default_profile: defaultProfile.value ?? null,
    }
    return api.updateInstance(updated)
  },
  { success: () => t('instanceEdit.saved') },
)

const saving = computed(() => saveAction.busy['*'])
/** True once the form differs from what is saved on the instance. */
const dirty = computed(() => loaded.value && baseline.value !== '' && formSnapshot() !== baseline.value)

async function onSave() {
  if (!formValid.value) return
  const ok = await saveAction.run()
  if (!ok) return
  await store.refreshInstances()
  void router.push({ name: 'instances' })
}

/** Cancel means "undo my edits", not "go back" — the header already goes back. */
function onCancel() {
  const inst = current.value
  if (!inst) return
  fillForm(inst)
  void nextTick(() => (baseline.value = formSnapshot()))
}

// --- Port ---------------------------------------------------------------------

function parsePort(raw: string): number | null {
  const text = raw.trim()
  if (!text) return null
  const n = Number(text)
  return Number.isInteger(n) && n >= 1 && n <= 65535 ? n : null
}

const portAction = useAction(() => api.setInstancePort(id.value, parsePort(portInput.value)), {
  success: (updated) =>
    updated.port
      ? t('instanceEdit.portSaved', { port: updated.port })
      : t('instanceEdit.portSavedRandom'),
})

async function applyPort() {
  const updated = await portAction.run()
  if (!updated) return
  portInput.value = updated.port ? String(updated.port) : ''
  await store.refreshInstances()
}

// --- Icon ---------------------------------------------------------------------

const iconUrl = ref<string | null>(null)
const iconInput = ref('')

async function loadIcon() {
  try {
    iconUrl.value = await api.readInstanceIcon(id.value)
  } catch {
    iconUrl.value = null
  }
}
onMounted(loadIcon)

const iconAction = useAction(
  async (source: string) => {
    await api.setInstanceIcon(id.value, source)
    return true
  },
  { success: () => t('instanceEdit.iconUpdated') },
)

const clearIconAction = useAction(async () => {
  await api.clearInstanceIcon(id.value)
  return true
})

async function applyIconInput() {
  if (!iconInput.value.trim()) return
  const ok = await iconAction.run(iconInput.value.trim())
  if (ok !== true) return
  iconInput.value = ''
  await loadIcon()
  await store.refreshInstances()
}

async function pickIconFile() {
  const { open } = await import('@tauri-apps/plugin-dialog')
  const file = await open({
    multiple: false,
    filters: [{ name: 'Image', extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'] }],
  })
  if (typeof file !== 'string') return
  const ok = await iconAction.run(file)
  if (ok !== true) return
  await loadIcon()
  await store.refreshInstances()
}

async function clearIcon() {
  const ok = await clearIconAction.run()
  if (ok !== true) return
  await loadIcon()
  await store.refreshInstances()
}

// --- Quick links --------------------------------------------------------------

const dirAction = useAction(() => api.openInstanceDirectory(id.value), {
  success: (path) => t('instanceEdit.dirOpened', { path }),
})
const logAction = useAction(() => api.openInstanceLog(id.value), {
  success: (path) => t('instanceEdit.logOpened', { path }),
})
const terminalAction = useAction(() => api.openInstanceTerminal(id.value), {
  success: (label) => t('home.terminalOpened', { label }),
})

// --- Delete -------------------------------------------------------------------

const deleteAction = useAction(async () => {
  await api.deleteInstance(id.value)
  return true
})

function confirmDelete() {
  if (!current.value) return
  Modal.confirm({
    title: t('instances.deleteTitle', { name: name.value }),
    content: t('instances.deleteContent', { name: name.value, home: homeLabel.value }),
    okText: t('instances.table.delete'),
    cancelText: t('common.cancel'),
    okButtonProps: { status: 'danger' },
    modalClass: 'dl-sheet',
    onOk: async () => {
      const ok = await deleteAction.run()
      if (ok !== true) return
      await store.refreshInstances()
      Message.success(t('instances.deleted', { name: name.value }))
      void router.push({ name: 'instances' })
    },
  })
}

function addEnvRow() {
  envRows.value.push({ key: '', value: '' })
}
function removeEnvRow(idx: number) {
  envRows.value.splice(idx, 1)
}
</script>

<template>
  <div v-if="loaded" class="dl-page detail-page">
    <header class="dl-head detail-head">
      <div class="dl-head-text">
        <h1 class="detail-title">
          <InstanceAvatar :id="id" :icon="current?.icon" :size="34" />
          <span class="detail-name">{{ name.trim() || t('instanceEdit.titleEdit') }}</span>
          <span class="dl-status" :class="`is-${status.state}`">
            {{ t(`home.status.${status.state}`) }}
          </span>
        </h1>
        <p class="dl-head-sub detail-sub">
          <span class="dl-mono">{{ store.versionById(versionId ?? '')?.version ?? '—' }}</span>
          <span class="detail-dot">·</span>
          <span>{{ homeLabel }}</span>
          <span class="detail-dot">·</span>
          <span class="dl-mono detail-id" :title="id">{{ id }}</span>
        </p>
      </div>
      <div class="dl-head-actions">
        <!-- Only a running instance has a plugin manager to talk to. -->
        <button v-if="canManagePlugins(id)" class="dl-btn dl-btn--quiet" @click="openPlugins(id, 'instance-detail')">
          <DlIcon name="plugin" :size="13" />
          {{ t('profiles.managePlugins') }}
        </button>
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
    </header>

    <!-- Two columns: what the instance *is* on the left, the things you attach
         to it on the right. Each row reads like the settings page — label and
         explanation left, control right — so the two forms in this app look
         like one app. -->
    <div class="detail-grid">
      <div class="detail-col">
        <section class="dl-section">
          <div class="dl-section-label">{{ t('instanceEdit.sectionBinding') }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('instanceEdit.name') }}</span>
              </div>
              <input
                v-model="name"
                class="dl-input detail-ctl"
                :placeholder="t('instanceEdit.namePlaceholder')"
              />
            </div>

            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('instanceEdit.version') }}</span>
              </div>
              <a-select v-model="versionId" style="width: 230px">
                <a-option v-for="v in store.versions" :key="v.id" :value="v.id">{{ v.version }}</a-option>
              </a-select>
            </div>

            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('instanceEdit.home') }}</span>
              </div>
              <a-select v-model="homeId" style="width: 230px" @change="onHomePicked">
                <a-option v-for="h in store.homes" :key="h.id" :value="h.id">
                  {{ h.name }} — {{ h.path }}
                </a-option>
              </a-select>
            </div>

            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('instanceEdit.defaultProfile') }}</span>
                <span class="dl-row-desc">{{ t('instanceEdit.defaultProfileHint') }}</span>
              </div>
              <a-select
                v-model="defaultProfile"
                style="width: 230px"
                :placeholder="t('instanceEdit.defaultProfilePlaceholder')"
                allow-clear
              >
                <a-option v-for="p in profiles" :key="p" :value="p">{{ p }}</a-option>
              </a-select>
            </div>
          </div>
        </section>

        <section class="dl-section">
          <div class="dl-section-label">{{ t('instanceEdit.sectionLaunch') }}</div>
          <div class="dl-surface">
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ t('instanceEdit.port') }}</span>
                <span class="dl-row-desc">{{ t('instanceEdit.portHint') }}</span>
              </div>
              <div class="detail-inline">
                <input
                  v-model="portInput"
                  class="dl-input dl-input--mono detail-port"
                  :placeholder="t('instanceEdit.portPlaceholder')"
                  @keydown.enter="applyPort"
                />
                <button class="dl-btn dl-btn--quiet" :disabled="portAction.busy['*']" @click="applyPort">
                  {{ t('instanceEdit.portApply') }}
                </button>
              </div>
            </div>
          </div>
        </section>
      </div>

      <div class="detail-col">
        <section class="dl-section">
          <div class="dl-section-label">{{ t('instanceEdit.icon') }}</div>
          <div class="dl-surface detail-pad">
            <div class="icon-block">
              <span class="icon-preview">
                <img v-if="iconUrl" :src="iconUrl" alt="" />
                <InstanceAvatar v-else :id="id" :icon="null" :size="46" class="icon-preview-fallback" />
              </span>
              <div class="icon-controls">
                <div class="detail-inline">
                  <input
                    v-model="iconInput"
                    class="dl-input icon-url"
                    :placeholder="t('instanceEdit.iconUrlHint')"
                  />
                  <button
                    class="dl-btn dl-btn--quiet"
                    :disabled="!iconInput.trim() || iconAction.busy['*']"
                    @click="applyIconInput"
                  >
                    {{ t('instanceEdit.iconApply') }}
                  </button>
                </div>
                <div class="detail-inline">
                  <button class="dl-btn dl-btn--quiet" :disabled="iconAction.busy['*']" @click="pickIconFile">
                    {{ t('instanceEdit.iconPickFile') }}
                  </button>
                  <button v-if="iconUrl" class="dl-btn dl-btn--danger" @click="clearIcon">
                    {{ t('instanceEdit.iconClear') }}
                  </button>
                </div>
              </div>
            </div>
          </div>
        </section>

        <section class="dl-section">
          <div class="dl-section-label">
            {{ t('instanceEdit.env') }}
            <span class="dl-section-count">{{ envRows.length }}</span>
          </div>
          <div class="dl-surface detail-pad">
            <p class="dl-row-desc env-desc">{{ t('instanceEdit.envDesc') }}</p>
            <div v-for="(row, idx) in envRows" :key="idx" class="env-row">
              <input
                v-model="row.key"
                class="dl-input dl-input--mono env-key"
                :class="{ 'dl-input--error': !!envKeyError(row) }"
                :placeholder="t('instanceEdit.envKey')"
              />
              <span class="env-eq">=</span>
              <input
                v-model="row.value"
                class="dl-input dl-input--mono env-value"
                :placeholder="t('instanceEdit.envValue')"
              />
              <button class="dl-icon-btn" @click="removeEnvRow(idx)">
                <DlIcon name="trash" :size="14" />
              </button>
              <span v-if="envKeyError(row)" class="dl-error-text env-error">{{ envKeyError(row) }}</span>
            </div>
            <button class="dl-btn dl-btn--quiet" @click="addEnvRow">
              <DlIcon name="plus" :size="13" />
              {{ t('instanceEdit.envAdd') }}
            </button>
          </div>
        </section>

        <section class="dl-section">
          <div class="dl-section-label">{{ t('instanceEdit.sectionDanger') }}</div>
          <div class="dl-surface detail-pad danger-card">
            <div class="danger-text">
              <span class="danger-title">{{ t('instances.deleteTitleShort') }}</span>
              <span class="dl-row-desc">{{ t('instances.deleteHint') }}</span>
            </div>
            <button class="dl-btn dl-btn--danger" :disabled="deleteAction.busy['*']" @click="confirmDelete">
              <DlIcon name="trash" :size="13" />
              {{ t('instances.table.delete') }}
            </button>
          </div>
        </section>
      </div>
    </div>

    <!-- Footer: the sticky save bar for the whole form. -->
    <div class="detail-footer">
      <span v-if="dirty" class="footer-dirty">{{ t('instanceEdit.unsaved') }}</span>
      <button class="dl-btn dl-btn--quiet" :disabled="!dirty || saving" @click="onCancel">
        {{ t('instanceEdit.cancel') }}
      </button>
      <button class="dl-btn dl-btn--primary" :disabled="!formValid || !dirty || saving" @click="onSave">
        {{ t('instanceEdit.save') }}
      </button>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.detail-page {
  padding-bottom: 96px;
}

.detail-title {
  display: flex;
  align-items: center;
  gap: 10px;
}

.detail-name {
  font-size: var(--dl-text-xl);
  font-weight: 650;
}

.detail-sub {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 3px;
}

.detail-dot {
  color: var(--dl-ink-4);
}

.detail-id {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 220px;
  color: var(--dl-ink-4);
}

// Left: what the instance is. Right: what is attached to it.
.detail-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr);
  gap: 0 28px;
  align-items: start;
}

// Controls keep a readable width instead of stretching across the card.
// Selects carry the width inline: Arco's root element never receives Vue's
// scope id, so a scoped rule can never match it.
.detail-ctl {
  width: 230px;
  flex-shrink: 0;
}

.detail-port {
  width: 120px;
}

.detail-pad {
  padding: 16px;
}

.detail-inline {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.icon-block {
  display: flex;
  align-items: center;
  gap: 14px;
}

.icon-preview {
  display: inline-flex;
  width: 46px;
  height: 46px;
  flex-shrink: 0;
  overflow: hidden;

  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: 10px;
  }

  .icon-preview-fallback {
    width: 46px;
    height: 46px;
  }
}

.icon-controls {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.icon-url {
  flex: 1 1 150px;
  min-width: 130px;
}

.env-desc {
  margin: 0 0 12px;
}

.env-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;

  .env-key {
    width: 150px;
  }

  .env-eq {
    color: var(--dl-ink-4);
    font-family: var(--dl-font-mono);
  }

  .env-value {
    flex: 1;
    min-width: 0;
  }
}

.env-error {
  flex-basis: 100%;
}

.danger-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  border-color: var(--dl-danger-soft);

  .danger-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .danger-title {
    font-size: var(--dl-text-md);
    font-weight: 600;
    color: var(--dl-ink);
  }
}

.detail-footer {
  position: sticky;
  bottom: 0;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 28px;
  padding: 14px 0;
  border-top: 1px solid var(--dl-line);
  background: linear-gradient(180deg, transparent, var(--dl-canvas) 30%);
}

.footer-dirty {
  margin-right: auto;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

// One column once the two side by side stop fitting.
@media (max-width: 1040px) {
  .detail-grid {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>

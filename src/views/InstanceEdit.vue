<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useAction } from '@/composables/useAction'
import { useLauncherStore } from '@/stores/launcher'
import type { DshInstance } from '@/api/types'

const route = useRoute()
const router = useRouter()
const { t } = useI18n()
const store = useLauncherStore()

const editingId = computed(() => (route.params.id as string | undefined) ?? null)

const name = ref('')
const versionId = ref<string | undefined>(undefined)
const DEDICATED = '__dedicated__'
const homeId = ref<string | undefined>(undefined)
const dedicatedPath = ref('')
const defaultProfile = ref<string | undefined>(undefined)
const profiles = ref<string[]>([])
const preserveSymlinks = ref(false)

// --- Node runtime flags -------------------------------------------------------

async function onPreserveSymlinksChange(value: unknown) {
  preserveSymlinks.value = Boolean(value)
}

// --- Web port -----------------------------------------------------------------

const portInput = ref('')

function parsePortInput(raw: string): number | null {
  const text = raw.trim()
  if (!text) return null
  const n = Number(text)
  return Number.isInteger(n) && n >= 1 && n <= 65535 ? n : null
}

const portAction = useAction(
  () => api.setInstancePort(editingId.value!, parsePortInput(portInput.value)),
  {
    success: (updated) =>
      updated.port
        ? t('instanceEdit.portSaved', { port: updated.port })
        : t('instanceEdit.portSavedRandom'),
  },
)
const portBusy = computed(() => portAction.busy['*'])

async function applyPort() {
  if (!editingId.value) return
  const updated = await portAction.run()
  if (updated === undefined) return
  const inst = store.instanceById(editingId.value)
  if (inst) inst.port = updated.port ?? null
  portInput.value = updated.port ? String(updated.port) : ''
}

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
  if (!editingId.value) return
  const inst = store.instanceById(editingId.value) ?? (await api.listInstances()).find((i) => i.id === editingId.value)
  if (!inst) {
    Message.error(t('instanceEdit.notFound'))
    router.replace({ name: 'home' })
    return
  }
  name.value = inst.name
  versionId.value = inst.version_id
  homeId.value = inst.home_id
  defaultProfile.value = inst.default_profile ?? undefined
  portInput.value = inst.port ? String(inst.port) : ''
  preserveSymlinks.value = inst.preserve_symlinks ?? false
  envRows.value = Object.entries(inst.env_overrides).map(([key, value]) => ({ key, value }))
  await loadIcon()
})

// --- Instance icon ------------------------------------------------------------

const iconUrl = ref<string | null>(null)
const iconInput = ref('')
const iconAction = useAction(
  async (source: string) => {
    await api.setInstanceIcon(editingId.value!, source)
    return true
  },
  { success: () => t('instanceEdit.iconUpdated') },
)
const iconBusy = computed(() => iconAction.busy['*'])

const clearIconAction = useAction(async () => {
  await api.clearInstanceIcon(editingId.value!)
  return true
})

async function loadIcon() {
  if (!editingId.value) return
  try {
    iconUrl.value = await api.readInstanceIcon(editingId.value)
  } catch {
    iconUrl.value = null
  }
}

async function applyIconInput() {
  if (!editingId.value || !iconInput.value.trim()) return
  const ok = await iconAction.run(iconInput.value.trim())
  if (ok !== true) return
  iconInput.value = ''
  await loadIcon()
  await store.refreshInstances()
}

async function pickIconFile() {
  if (!editingId.value) return
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
  if (!editingId.value) return
  const ok = await clearIconAction.run()
  if (ok !== true) return
  await loadIcon()
  await store.refreshInstances()
}

watch(homeId, async (v) => {
  profiles.value = []
  if (v === DEDICATED) {
    dedicatedPath.value = await api.defaultDedicatedHomePath(name.value.trim() || 'instance')
    return
  }
  if (!v) return
  try {
    profiles.value = await api.listProfiles(v)
    if (defaultProfile.value && !profiles.value.includes(defaultProfile.value)) {
      defaultProfile.value = undefined
    }
  } catch (e) {
    Message.error(String(e))
  }
})

watch(name, async (v) => {
  if (homeId.value === DEDICATED) {
    dedicatedPath.value = await api.defaultDedicatedHomePath(v.trim() || 'instance')
  }
})

const formValid = computed(
  () => name.value.trim().length > 0 && !!versionId.value && !!homeId.value && envValid.value,
)

const saveAction = useAction(
  async () => {
    const envOverrides: Record<string, string> = {}
    for (const row of envRows.value) {
      if (row.key) envOverrides[row.key] = row.value
    }
    let resolvedHomeId = homeId.value!
    if (homeId.value === DEDICATED) {
      const home = await api.createHome(name.value.trim(), dedicatedPath.value)
      resolvedHomeId = home.id
      await store.refreshHomes()
    }
    const inst = store.instanceById(editingId.value!) as DshInstance
    await api.updateInstance({
      ...inst,
      name: name.value.trim(),
      version_id: versionId.value!,
      home_id: resolvedHomeId,
      env_overrides: envOverrides,
      default_profile: defaultProfile.value ?? null,
      preserve_symlinks: preserveSymlinks.value,
    })
    await store.refreshInstances()
    return true
  },
  { success: () => t('instanceEdit.saved') },
)
const saving = computed(() => saveAction.busy['*'])

async function onSave() {
  if (!formValid.value) return
  const ok = await saveAction.run()
  if (ok !== true) return
  router.push({ name: 'home' })
}

function addEnvRow() {
  envRows.value.push({ key: '', value: '' })
}

function removeEnvRow(idx: number) {
  envRows.value.splice(idx, 1)
}

const dirAction = useAction(() => api.openInstanceDirectory(editingId.value!), {
  success: (path) => t('instanceEdit.dirOpened', { path }),
})
const logAction = useAction(() => api.openInstanceLog(editingId.value!), {
  success: (path) => t('instanceEdit.logOpened', { path }),
})
const dirBusy = computed(() => dirAction.busy['*'])
const logBusy = computed(() => logAction.busy['*'])

async function onOpenDirectory() {
  if (!editingId.value) return
  await dirAction.run()
}

async function onViewLog() {
  if (!editingId.value) return
  await logAction.run()
}

const homeLabel = computed(() => {
  if (!homeId.value) return '—'
  if (homeId.value === DEDICATED) return t('instanceEdit.dedicatedHome')
  return store.homeById(homeId.value)?.name ?? homeId.value
})
</script>

<template>
  <div class="dl-page edit-page">
    <!-- Clean Inspector Card -->
    <div class="dl-card edit-container-card">
      <!-- Top Profile Overview -->
      <div class="edit-overview-header">
        <div class="overview-avatar">
          <img v-if="iconUrl" :src="iconUrl" alt="" />
          <img v-else src="@/assets/launcher-icon.png" alt="" />
        </div>
        <div class="overview-info">
          <div class="overview-title">{{ name.trim() || t('instanceEdit.titleEdit') }}</div>
          <div class="overview-subtitle tnum">
            {{ store.versionById(versionId ?? '')?.version ?? '—' }} · {{ homeLabel }}
            <template v-if="defaultProfile"> · {{ defaultProfile }}</template>
          </div>
        </div>
        <div class="overview-right-actions">
          <button
            v-if="editingId"
            type="button"
            class="mac-secondary-btn"
            :disabled="dirBusy"
            @click="onOpenDirectory"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
              <path d="M2 4a1.5 1.5 0 0 1 1.5-1.5h3l2 2H13a1.5 1.5 0 0 1 1.5 1.5v6a1.5 1.5 0 0 1-1.5 1.5H3.5A1.5 1.5 0 0 1 2 12V4z" />
            </svg>
            <span>{{ t('instanceEdit.openDir') }}</span>
          </button>
          <button
            v-if="editingId"
            type="button"
            class="mac-secondary-btn"
            :disabled="logBusy"
            @click="onViewLog"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
              <rect x="3" y="2" width="10" height="12" rx="1.5" />
              <line x1="6" y1="6" x2="10" y2="6" />
              <line x1="6" y1="9" x2="10" y2="9" />
            </svg>
            <span>{{ t('instanceEdit.viewLog') }}</span>
          </button>
          <span v-if="editingId" class="overview-id-chip tnum">{{ editingId.slice(0, 8) }}</span>
        </div>
      </div>

      <!-- Form Body -->
      <a-form layout="vertical" class="apple-edit-form" :model="{}">
        <a-form-item :label="t('instanceEdit.name')" required>
          <a-input v-model="name" :placeholder="t('instanceEdit.namePlaceholder')" style="max-width: 420px" disabled />
        </a-form-item>

        <a-form-item v-if="editingId" :label="t('instanceEdit.icon')">
          <div class="icon-editor-block">
            <div class="icon-avatar-preview">
              <img v-if="iconUrl" :src="iconUrl" alt="" />
              <img v-else src="@/assets/launcher-icon.png" alt="" />
            </div>
            <div class="icon-ctrl-wrap">
              <div class="icon-url-row">
                <a-input
                  v-model="iconInput"
                  :placeholder="t('instanceEdit.iconUrlHint')"
                  allow-clear
                  style="max-width: 300px"
                />
                <button
                  type="button"
                  class="mac-secondary-btn"
                  :disabled="!iconInput.trim() || iconBusy"
                  @click="applyIconInput"
                >
                  {{ t('instanceEdit.iconApply') }}
                </button>
              </div>
              <div class="icon-actions-row">
                <button type="button" class="mac-secondary-btn" :disabled="iconBusy" @click="pickIconFile">
                  {{ t('instanceEdit.iconPickFile') }}
                </button>
                <button v-if="iconUrl" type="button" class="mac-action-pill danger" @click="clearIcon">
                  {{ t('instanceEdit.iconClear') }}
                </button>
              </div>
            </div>
          </div>
        </a-form-item>

        <a-form-item :label="t('instanceEdit.version')" required>
          <a-select v-model="versionId" style="max-width: 420px" disabled>
            <a-option v-for="v in store.versions" :key="v.id" :value="v.id">{{ v.version }}</a-option>
          </a-select>
        </a-form-item>

        <a-form-item :label="t('instanceEdit.home')" required>
          <a-select v-model="homeId" style="max-width: 420px">
            <a-option :value="DEDICATED">{{ t('instanceEdit.dedicatedHome') }}</a-option>
            <a-option v-for="h in store.homes" :key="h.id" :value="h.id">
              {{ h.name }}（{{ h.path }}）
            </a-option>
          </a-select>
          <p v-if="homeId === DEDICATED" class="field-hint-text">
            {{ t('instanceEdit.dedicatedHomeHint', { path: dedicatedPath }) }}
          </p>
        </a-form-item>

        <a-form-item
          v-if="homeId && homeId !== DEDICATED"
          :label="t('instanceEdit.defaultProfile')"
        >
          <a-select
            v-model="defaultProfile"
            :placeholder="t('instanceEdit.defaultProfilePlaceholder')"
            allow-clear
            style="max-width: 420px"
          >
            <a-option v-for="p in profiles" :key="p" :value="p">{{ p }}</a-option>
          </a-select>
          <p class="field-hint-text">{{ t('instanceEdit.defaultProfileHint') }}</p>
        </a-form-item>

        <a-form-item v-if="editingId" :label="t('instanceEdit.port')">
          <div class="port-row">
            <a-input
              v-model="portInput"
              :placeholder="t('instanceEdit.portPlaceholder')"
              allow-clear
              style="width: 180px"
              @press-enter="applyPort"
            />
            <button type="button" class="mac-secondary-btn" :disabled="portBusy" @click="applyPort">
              {{ t('instanceEdit.portApply') }}
            </button>
          </div>
        </a-form-item>

        <a-form-item :label="t('instanceEdit.preserveSymlinks')">
          <div class="port-row">
            <a-switch
              :model-value="preserveSymlinks"
              size="small"
              @change="onPreserveSymlinksChange"
            />
          </div>
          <p class="field-hint-text">{{ t('instanceEdit.preserveSymlinksHint') }}</p>
        </a-form-item>

        <a-form-item :label="t('instanceEdit.env')">
          <p class="field-hint-text">{{ t('instanceEdit.envDesc') }}</p>
          <div class="env-editor-wrap">
            <div v-for="(row, idx) in envRows" :key="idx" class="env-item-row">
              <input
                v-model="row.key"
                :placeholder="t('instanceEdit.envKey')"
                class="apple-input-sm env-k"
                :class="{ 'has-error': !!envKeyError(row) } "
              />
              <input
                v-model="row.value"
                :placeholder="t('instanceEdit.envValue')"
                class="apple-input-sm env-v"
              />
              <button type="button" class="mac-micro-btn danger" @click="removeEnvRow(idx)">
                {{ t('instances.table.delete') }}
              </button>
              <div v-if="envKeyError(row)" class="env-error-chip">{{ envKeyError(row) }}</div>
            </div>
            <button type="button" class="mac-secondary-btn add-env-btn" @click="addEnvRow">
              + {{ t('instanceEdit.envAdd') }}
            </button>
          </div>
        </a-form-item>
      </a-form>

      <!-- Footer Buttons -->
      <div class="edit-footer-bar">
        <button type="button" class="mac-secondary-btn" @click="router.push({ name: 'home' })">
          {{ t('instanceEdit.cancel') }}
        </button>
        <button
          type="button"
          class="mac-primary-btn"
          :disabled="!formValid || saving"
          @click="onSave"
        >
          {{ t('instanceEdit.save') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.edit-page {
  max-width: 800px;
  margin: 0 auto;
}

.edit-container-card {
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.edit-overview-header {
  display: flex;
  align-items: center;
  gap: 14px;
  padding-bottom: 16px;
  border-bottom: 1px solid var(--apple-separator);
}

.overview-avatar {
  width: 44px;
  height: 44px;
  border-radius: 11px;
  overflow: hidden;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.12);
  flex-shrink: 0;

  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
}

.overview-info {
  flex: 1;
  min-width: 0;

  .overview-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--color-text-1);
    letter-spacing: -0.015em;
  }

  .overview-subtitle {
    font-size: 12px;
    color: var(--color-text-3);
    margin-top: 2px;
  }
}

.overview-right-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.overview-id-chip {
  padding: 2px 8px;
  font-size: 11.5px;
  border-radius: 6px;
  background: var(--apple-group-bg);
  color: var(--color-text-3);
}

.icon-editor-block {
  display: flex;
  align-items: flex-start;
  gap: 16px;

  .icon-avatar-preview {
    width: 54px;
    height: 54px;
    border-radius: 12px;
    overflow: hidden;
    border: 1px solid var(--apple-card-border);
    flex-shrink: 0;

    img {
      width: 100%;
      height: 100%;
      object-fit: cover;
    }
  }

  .icon-ctrl-wrap {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .icon-url-row,
  .icon-actions-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
}

.field-hint-text {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--color-text-3);
  line-height: 1.5;
}

.port-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.apple-input-sm {
  height: 30px;
  padding: 0 10px;
  font-size: 13px;
  border-radius: 7px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-group-bg);
  color: var(--color-text-1);
  outline: none;
  transition: all 0.16s ease;

  &:focus {
    background: var(--apple-card-bg);
    border-color: rgb(var(--primary-6));
    box-shadow: 0 0 0 2px rgb(var(--primary-6) / 18%);
  }

  &.has-error {
    border-color: rgb(var(--red-6));
  }
}

.env-editor-wrap {
  display: flex;
  flex-direction: column;
  gap: 8px;

  .env-item-row {
    display: flex;
    align-items: center;
    gap: 8px;

    .env-k {
      width: 160px;
      font-family: 'SF Mono', monospace;
    }

    .env-v {
      flex: 1;
      max-width: 320px;
      font-family: 'SF Mono', monospace;
    }
  }

  .add-env-btn {
    align-self: flex-start;
  }
}

.env-error-chip {
  font-size: 11px;
  color: rgb(var(--red-6));
}

.edit-footer-bar {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  padding-top: 16px;
  border-top: 1px solid var(--apple-separator);
}

.mac-primary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 32px;
  padding: 0 16px;
  font-size: 13px;
  font-weight: 500;
  border-radius: 8px;
  border: none;
  background: rgb(var(--primary-6));
  color: #fff;
  cursor: pointer;
  transition: all 0.16s ease;

  &:hover:not(:disabled) {
    filter: brightness(1.06);
  }

  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  &:active:not(:disabled) {
    transform: scale(var(--apple-active-scale));
  }
}

.mac-secondary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 12px;
  font-size: 12.5px;
  font-weight: 500;
  border-radius: 7px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  cursor: pointer;
  transition: all 0.16s ease;

  &:hover:not(:disabled) {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  &:active:not(:disabled) {
    transform: scale(var(--apple-active-scale));
  }
}

.mac-micro-btn {
  padding: 3px 8px;
  font-size: 12px;
  border-radius: 6px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--color-text-2);
  cursor: pointer;
  transition: all 0.15s ease;

  &.danger {
    color: rgb(var(--red-6));

    &:hover {
      background: rgb(var(--red-6) / 12%);
    }
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.mac-action-pill {
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  border-radius: 6px;
  padding: 3px 9px;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.15s ease;

  &.danger {
    color: rgb(var(--red-6));

    &:hover {
      background: rgb(var(--red-6) / 12%);
    }
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}
</style>

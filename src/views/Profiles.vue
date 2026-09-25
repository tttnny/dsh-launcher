<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import type { DshInstance } from '@/api/types'
import { useAction } from '@/composables/useAction'
import DlEmpty from '@/components/DlEmpty.vue'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useLauncherStore()

// --- Selected HOME -----------------------------------------------------------

const selectedHomeId = ref<string | undefined>(undefined)

const selectedHome = computed(() => store.homeById(selectedHomeId.value ?? ''))

// Referential integrity comes from the store getter, not a per-view copy.
const instancesOfHome = store.instancesOfHome

// Watch route.query or store.homes to initialize / sync selectedHomeId
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

function onSelectHome(id: unknown) {
  const homeId = String(id ?? '')
  selectedHomeId.value = homeId
  void router.replace({ query: { ...route.query, homeId } })
}

// --- Profiles of the selected HOME -------------------------------------------

const profiles = ref<string[]>([])
const profilesLoading = ref(false)
const newProfileName = ref('')
const addingProfile = ref(false)
const renamingProfile = ref<string | null>(null)
const renameValue = ref('')
const copyingProfile = ref<string | null>(null)
const copyProfileName = ref('')

async function loadProfiles() {
  profiles.value = []
  if (!selectedHomeId.value) return
  profilesLoading.value = true
  try {
    profiles.value = await api.listProfiles(selectedHomeId.value)
  } catch (e) {
    Message.error(String(e))
  } finally {
    profilesLoading.value = false
  }
}

watch(
  selectedHomeId,
  () => {
    void loadProfiles()
  },
  { immediate: true },
)

const createAction = useAction((homeId: string, name: string) => api.createProfile(homeId, name), {
  success: (name) => t('profiles.profileCreated', { name }),
})
const creatingProfile = computed(() => createAction.busy['*'])

async function onCreateProfile() {
  const name = newProfileName.value.trim()
  if (!selectedHomeId.value || !name) return
  const created = await createAction.run(selectedHomeId.value, name)
  if (created === undefined) return
  newProfileName.value = ''
  addingProfile.value = false
  await loadProfiles()
}

function startRenameProfile(name: string) {
  renamingProfile.value = name
  renameValue.value = name
}

const renameAction = useAction(async (homeId: string, oldName: string, newName: string) => {
  const renamed = await api.renameProfile(homeId, oldName, newName)
  return { old: oldName, name: renamed }
}, {
  key: (_homeId, oldName) => oldName,
  success: ({ old, name }) => t('profiles.profileRenamed', { old, name }),
})
const renameBusy = renameAction.busy

async function confirmRenameProfile() {
  const old = renamingProfile.value
  const name = renameValue.value.trim()
  if (!selectedHomeId.value || !old || !name || old === name) {
    renamingProfile.value = null
    return
  }
  const res = await renameAction.run(selectedHomeId.value, old, name)
  if (res === undefined) return
  await store.refreshInstances()
  await loadProfiles()
  renamingProfile.value = null
}

function startCopyProfile(name: string) {
  copyingProfile.value = name
  copyProfileName.value = `${name}-copy`
}

const copyAction = useAction(async (homeId: string, source: string, name: string) => {
  const copied = await api.copyProfile(homeId, source, name)
  return { source, name: copied }
}, {
  success: ({ source, name }) => t('profiles.profileCopied', { source, name }),
})
const copyProfileBusy = computed(() => copyAction.busy['*'])

async function confirmCopyProfile() {
  const source = copyingProfile.value
  const name = copyProfileName.value.trim()
  if (!selectedHomeId.value || !source || !name) {
    copyingProfile.value = null
    return
  }
  const res = await copyAction.run(selectedHomeId.value, source, name)
  if (res === undefined) return
  await loadProfiles()
  copyingProfile.value = null
}

const deleteAction = useAction(async (homeId: string, name: string) => {
  await api.deleteProfile(homeId, name)
  return name
}, {
  key: (_homeId, name) => name,
  success: (name) => t('profiles.profileDeleted', { name }),
})
const deleteBusy = deleteAction.busy

async function confirmDeleteProfile(name: string) {
  if (!selectedHomeId.value) return
  const deleted = await deleteAction.run(selectedHomeId.value, name)
  if (deleted === undefined) return
  await store.refreshInstances()
  await loadProfiles()
}

const setDefaultAction = useAction(async (inst: DshInstance, profile: string) => {
  await api.updateInstance({ ...inst, default_profile: profile })
  return profile
}, {
  key: (inst) => inst.id,
  success: (profile) => t('profiles.profileSetDefault', { name: profile }),
})

async function setDefaultProfile(instanceId: string, profile: string) {
  const inst = store.instanceById(instanceId)
  if (!inst) return
  const set = await setDefaultAction.run(inst, profile)
  if (set === undefined) return
  await store.refreshInstances()
}

function onManagePlugins(profile: string) {
  if (!selectedHomeId.value) return
  void router.push({
    path: '/plugins',
    query: {
      homeId: selectedHomeId.value,
      profile,
    },
  })
}
</script>

<template>
  <div class="dl-page profiles-page">
    <!-- Top Selector Bar -->
    <div class="dl-card home-selector-bar">
      <div class="selector-row">
        <label class="selector-label">{{ t('profiles.selectHome') }}：</label>
        <a-select
          :model-value="selectedHomeId"
          :placeholder="t('profiles.selectHome')"
          class="home-select"
          @change="onSelectHome"
        >
          <a-option v-for="h in store.homes" :key="h.id" :value="h.id">
            {{ h.name }} ({{ h.path }})
          </a-option>
        </a-select>
      </div>
    </div>

    <!-- Profiles Management Card -->
    <div v-if="selectedHomeId" class="dl-card profile-mgmt-card">
      <div class="dl-card-title">
        <div class="title-with-pill">
          <h3>{{ t('profiles.profilesTitle') }}</h3>
          <span v-if="selectedHome" class="home-tag-pill">{{ selectedHome.name }}</span>
        </div>
      </div>
      <p class="dl-card-desc">{{ t('profiles.profilesDesc') }}</p>

      <!-- Instances referencing this home -->
      <div v-if="instancesOfHome(selectedHomeId).length > 0" class="used-by-pill-box">
        <span class="used-by-label">{{ t('profiles.usedByInstances') }}：</span>
        <div class="used-by-list">
          <span v-for="inst in instancesOfHome(selectedHomeId)" :key="inst.id" class="used-by-item">
            <span class="inst-chip-name">{{ inst.name }}</span>
            <span class="inst-chip-desc">（{{ t('profiles.defaultOf', { instance: inst.name }) }}：{{ inst.default_profile ?? t('profiles.noDefault') }}）</span>
            <a-select
              :model-value="inst.default_profile ?? undefined"
              :placeholder="t('profiles.noDefault')"
              allow-clear
              size="mini"
              style="width: 130px"
              @change="(v: unknown) => v && setDefaultProfile(inst.id, String(v))"
            >
              <a-option v-for="p in profiles" :key="p" :value="p">{{ p }}</a-option>
            </a-select>
          </span>
        </div>
      </div>
      <p v-else class="dl-card-desc">{{ t('profiles.noInstances') }}</p>

      <div v-if="profilesLoading" class="dl-card-desc">{{ t('common.loading') }}</div>
      <DlEmpty v-else-if="profiles.length === 0" icon="profile" :description="t('profiles.profilesEmpty')" />

      <!-- Profile Items List -->
      <div class="profile-items-group">
        <div v-for="p in profiles" :key="p" class="profile-item-row">
          <template v-if="renamingProfile === p">
            <input v-model="renameValue" class="apple-input-sm" @press-enter="confirmRenameProfile" />
            <div class="inline-btn-group">
              <button class="mac-primary-btn" :disabled="renameBusy[p] || deleteBusy[p]" @click="confirmRenameProfile">
                {{ t('profiles.profileRenameSave') }}
              </button>
              <button class="mac-secondary-btn" @click="renamingProfile = null">
                {{ t('common.cancel') }}
              </button>
            </div>
          </template>

          <template v-else-if="copyingProfile === p">
            <input v-model="copyProfileName" class="apple-input-sm" @press-enter="confirmCopyProfile" />
            <div class="inline-btn-group">
              <button class="mac-primary-btn" :disabled="copyProfileBusy" @click="confirmCopyProfile">
                {{ t('profiles.profileCopySave') }}
              </button>
              <button class="mac-secondary-btn" @click="copyingProfile = null">
                {{ t('common.cancel') }}
              </button>
            </div>
          </template>

          <template v-else>
            <div class="profile-left-col">
              <span class="profile-title">{{ p }}</span>
            </div>
            <div class="profile-item-actions">
              <button class="mac-action-pill highlight" @click="onManagePlugins(p)">
                {{ t('profiles.managePlugins') }}
              </button>
              <button class="mac-micro-btn" @click="startRenameProfile(p)">{{ t('profiles.profileRename') }}</button>
              <button class="mac-micro-btn" @click="startCopyProfile(p)">{{ t('profiles.profileCopy') }}</button>
              <a-popconfirm
                :content="t('profiles.profileDeleteConfirm', { name: p })"
                @ok="confirmDeleteProfile(p)"
              >
                <button class="mac-micro-btn danger" :disabled="renameBusy[p] || deleteBusy[p]">
                  {{ t('instances.table.delete') }}
                </button>
              </a-popconfirm>
            </div>
          </template>
        </div>

        <div v-if="addingProfile" class="profile-item-row is-editing">
          <input
            v-model="newProfileName"
            :placeholder="t('profiles.profileCreatePlaceholder')"
            class="apple-input-sm"
            @press-enter="onCreateProfile"
          />
          <div class="inline-btn-group">
            <button class="mac-primary-btn" :disabled="creatingProfile" @click="onCreateProfile">
              {{ t('profiles.profileCreate') }}
            </button>
            <button class="mac-secondary-btn" @click="addingProfile = false">
              {{ t('common.cancel') }}
            </button>
          </div>
        </div>
      </div>

      <button v-if="!addingProfile" class="mac-secondary-btn" style="margin-top: 12px" @click="addingProfile = true">
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="8" y1="3" x2="8" y2="13" />
          <line x1="3" y1="8" x2="13" y2="8" />
        </svg>
        <span>{{ t('profiles.profileAdd') }}</span>
      </button>
    </div>

    <!-- Empty Home state -->
    <div v-else class="dl-card">
      <DlEmpty icon="profile" :description="t('profiles.noHomeSelected')" />
    </div>
  </div>
</template>

<style lang="scss" scoped>
.profiles-page {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.home-selector-bar {
  padding: 14px 18px;

  .selector-row {
    display: flex;
    align-items: center;
    gap: 12px;

    .selector-label {
      font-size: 13px;
      font-weight: 500;
      color: var(--color-text-2);
      white-space: nowrap;
    }

    .home-select {
      max-width: 420px;
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

.home-tag-pill {
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

.used-by-pill-box {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-bottom: 14px;
  padding: 10px 12px;
  border-radius: 8px;
  background: var(--apple-group-bg);
  border: 1px solid var(--apple-card-border);

  .used-by-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--color-text-3);
  }

  .used-by-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .used-by-item {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;

    .inst-chip-name {
      font-weight: 500;
      color: var(--color-text-1);
    }

    .inst-chip-desc {
      color: var(--color-text-3);
      font-size: 12px;
    }
  }
}

.profile-items-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 12px;
}

.profile-item-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  border-radius: 8px;
  background: var(--apple-group-bg);
  border: 1px solid var(--apple-card-border);
  gap: 12px;

  &.is-editing {
    background: var(--apple-card-bg);
  }

  .profile-left-col {
    display: flex;
    align-items: center;
    gap: 10px;

    .profile-title {
      font-size: 13px;
      font-weight: 500;
      color: var(--color-text-1);
    }
  }

  .profile-item-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .inline-btn-group {
    display: flex;
    align-items: center;
    gap: 8px;
  }
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
}

.mac-primary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 12px;
  font-size: 12.5px;
  font-weight: 500;
  border-radius: 7px;
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
  padding: 0 10px;
  font-size: 12.5px;
  font-weight: 500;
  border-radius: 7px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  cursor: pointer;
  transition: all 0.16s ease;

  &:hover {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
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

  &:hover:not(:disabled) {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &.highlight {
    color: rgb(var(--primary-6));
    border-color: rgb(var(--primary-6) / 30%);
    background: rgb(var(--primary-6) / 8%);

    &:hover:not(:disabled) {
      background: rgb(var(--primary-6) / 16%);
    }
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

  &:hover:not(:disabled) {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
    border-color: var(--apple-card-border);
  }

  &.danger {
    color: rgb(var(--red-6));

    &:hover:not(:disabled) {
      background: rgb(var(--red-6) / 12%);
    }
  }

  &:active:not(:disabled) {
    transform: scale(var(--apple-active-scale));
  }
}
</style>

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
import Plugins from '@/views/Plugins.vue'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useLauncherStore()

// --- Selected HOME -----------------------------------------------------------

const selectedHomeId = ref<string | undefined>(undefined)

const selectedHome = computed(() => store.homeById(selectedHomeId.value ?? ''))

// Referential integrity comes from the store getter, not a per-view copy.
const instancesOfHome = store.instancesOfHome

/**
 * The instances that use one Profile of the selected HOME. Each row shows its
 * own users, because "which instance starts with this Profile" is the only
 * reason the default-Profile picker exists; a HOME-wide list made the reader
 * match names by eye.
 */
function usersOf(profile: string): DshInstance[] {
  if (!selectedHomeId.value) return []
  return instancesOfHome(selectedHomeId.value).filter(
    (i) => (i.default_profile ?? i.last_profile) === profile,
  )
}

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

// The plugin manager lives *inside* the Profile list, and `/plugins` is a hash
// link onto this same page with `profile` naming the row it was opened from.
// The route is a claim about what is on screen, so everything below derives
// from it: leaving out of the query is leaving the manager, and the row's own
// button is just a way of writing that query.
const openProfile = computed(() =>
  route.name === 'plugins' && typeof route.query.profile === 'string'
    ? route.query.profile
    : '',
)

function onManagePlugins(profile: string) {
  if (!selectedHomeId.value) return
  const query = { homeId: selectedHomeId.value, profile }
  if (route.name === 'plugins') {
    // Switching Profile inside the manager: same route, different query. A
    // same-location `push` is reported as a duplicated navigation and dropped,
    // so the switch has to replace.
    void router.replace({ name: 'plugins', query })
    return
  }
  void router.push({ name: 'plugins', query })
}

/** Leaves the manager without leaving the list: the route drops the Profile. */
function onClosePlugins() {
  const rest = { ...route.query }
  delete rest.profile
  void router.replace({ name: 'plugins', query: rest })
}

// `homeId` without `profile` is a link onto the page rather than onto a row, so
// it lands on the plain list: the Profile the caller named may not exist here,
// and a manager pointed at the wrong HOME would be worse than one more click.
watch(
  () => route.query.profile,
  (profile) => {
    if (route.name !== 'plugins' || typeof profile !== 'string') return
    if (profiles.value.length > 0 && profiles.value.includes(profile)) return
    const rest = { ...route.query }
    delete rest.profile
    void router.replace({ name: 'plugins', query: rest })
  },
)
</script>

<template>
  <div class="dl-page profiles-page">
    <!-- Page heading -->
    <header class="dl-page-head">
      <h2>{{ t('profiles.title') }}</h2>
      <p class="dl-page-sub">{{ t('profiles.pageDesc') }}</p>
    </header>

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
            {{ h.name }}
          </a-option>
        </a-select>
        <!-- The path is identity, not a name: it rides beside the selector so
             the chosen HOME is unambiguous without widening the dropdown. -->
        <span v-if="selectedHome" class="dl-chip">{{ selectedHome.path }}</span>
      </div>
    </div>

    <!-- Profiles Management Card. While a Profile's plugin manager is open the
         list stays on screen — it is the navigation context of the manager, and
         the manager is opened *from* here — but it steps back: the manager
         below is the page's subject then. -->
    <div v-if="selectedHomeId" class="dl-card profile-mgmt-card" :class="{ 'is-context': openProfile }">
      <div class="dl-card-title">
        <div class="title-with-pill">
          <h3>{{ t('profiles.profilesTitle') }}</h3>
          <span v-if="selectedHome" class="home-tag-pill">{{ selectedHome.name }}</span>
        </div>
      </div>
      <p class="dl-card-desc">{{ t('profiles.profilesDesc') }}</p>

      <div v-if="profilesLoading" class="dl-card-desc">{{ t('common.loading') }}</div>
      <DlEmpty v-else-if="profiles.length === 0" icon="profile" :description="t('profiles.profilesEmpty')" />

      <!-- Profile Items List. A Profile used by instances is one row: what it
           is, who uses it, and what can be done with it. The default-Profile
           picker belongs to the instance that owns it, so it sits with that
           instance's name inside the row rather than in a separate block. -->
      <div class="profile-items-group">
        <div
          v-for="p in profiles"
          :key="p"
          class="profile-item-row"
          :class="{ 'is-open': openProfile === p }"
        >
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
              <div v-if="usersOf(p).length > 0" class="profile-users">
                <span v-for="inst in usersOf(p)" :key="inst.id" class="profile-user">
                  <span class="profile-user-name">{{ inst.name }}</span>
                  <a-select
                    :model-value="inst.default_profile ?? undefined"
                    :placeholder="t('profiles.noDefault')"
                    allow-clear
                    size="mini"
                    style="width: 128px"
                    @change="(v: unknown) => v && setDefaultProfile(inst.id, String(v))"
                  >
                    <a-option v-for="opt in profiles" :key="opt" :value="opt">{{ opt }}</a-option>
                  </a-select>
                </span>
              </div>
              <span v-else class="profile-no-user">{{ t('profiles.noInstances') }}</span>
            </div>
            <div class="profile-item-actions">
              <!-- The row's own action states which way it goes: it opens the
                   manager, and closes it again. The manager itself is rendered
                   by the child card below, under the same route. -->
              <button
                v-if="openProfile === p"
                class="mac-action-pill"
                @click="onClosePlugins"
              >
                {{ t('profiles.closePlugins') }}
              </button>
              <button v-else class="mac-action-pill highlight" @click="onManagePlugins(p)">
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

    <!-- The open Profile's plugin manager, under the route that names it.
         Rendered here rather than as a page of its own so switching Profile
         inside it costs a query change, not a round trip through the list. -->
    <Plugins
      v-if="openProfile"
      :profile="openProfile"
      :home-id="selectedHomeId ?? ''"
      @close="onClosePlugins"
    />
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
    min-width: 0;

    .selector-label {
      font-size: 13px;
      font-weight: 500;
      color: var(--color-text-2);
      white-space: nowrap;
    }

    .home-select {
      max-width: 320px;
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
  background: rgba(var(--primary-6), 0.1);
  color: rgb(var(--primary-6));
  border: 1px solid rgba(var(--primary-6), 0.2);
}

.profile-items-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 12px;
}

.profile-mgmt-card.is-context {
  opacity: 0.86;
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

  /* The row whose plugin manager is open below. It has to out-specify the
     row's own base rule, so the class is doubled rather than nested inside
     `.profile-item-row` — `&.is-open` compiles to one class + one attribute
     selector on the same element, which loses to `.profile-item-row[data-v-…]`
     by source order (the base rule comes first) and silently painted nothing. */
  &.is-open.is-open {
    border-color: rgba(var(--primary-6), 0.35);
    background: rgba(var(--primary-6), 0.06);
  }

  .profile-left-col {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    min-width: 0;

    .profile-title {
      font-size: 13.5px;
      font-weight: 600;
      color: var(--color-text-1);
    }
  }

  .profile-item-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .inline-btn-group {
    display: flex;
    align-items: center;
    gap: 8px;
  }
}

/* Which instances start with this Profile, one chip each: the name carries the
   meaning, the picker next to it sets that instance's default. */
.profile-users {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}

.profile-user {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 2px 6px 2px 10px;
  border-radius: 7px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);

  .profile-user-name {
    font-size: 12px;
    font-weight: 500;
    color: var(--color-text-2);
  }
}

.profile-no-user {
  font-size: 12px;
  color: var(--color-text-4);
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
    box-shadow: 0 0 0 2px rgba(var(--primary-6), 0.18);
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
    border-color: rgba(var(--primary-6), 0.3);
    background: rgba(var(--primary-6), 0.08);

    &:hover:not(:disabled) {
      background: rgba(var(--primary-6), 0.16);
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
      background: rgba(var(--red-6), 0.12);
    }
  }

  &:active:not(:disabled) {
    transform: scale(var(--apple-active-scale));
  }
}
</style>

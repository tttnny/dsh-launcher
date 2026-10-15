<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import type { DshInstance } from '@/api/types'
import { useAction } from '@/composables/useAction'
import DlIcon from '@/components/DlIcon.vue'
import DlEmpty from '@/components/DlEmpty.vue'

// A Profile is a plugin composition inside one DSH_HOME. This pane lists the
// selected HOME's profiles and says, on each row, which instances launch with
// it — the two facts (what it is, who uses it) that decide whether it is safe
// to rename, copy or delete. Managing its plugins opens the manager page.

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useLauncherStore()

const selectedHomeId = ref<string | undefined>(undefined)
const selectedHome = computed(() => store.homeById(selectedHomeId.value ?? ''))
const instancesOfHome = store.instancesOfHome

watch(
  [() => route.query.homeId, () => store.homes],
  ([qHomeId]) => {
    const q = typeof qHomeId === 'string' ? qHomeId : undefined
    if (q && store.homes.some((h) => h.id === q)) selectedHomeId.value = q
    else if (!selectedHomeId.value || !store.homes.some((h) => h.id === selectedHomeId.value)) {
      selectedHomeId.value = store.homes[0]?.id
    }
  },
  { immediate: true },
)

function onSelectHome(id: string) {
  selectedHomeId.value = id
  void router.replace({ name: 'library', query: { ...route.query, tab: 'profiles', homeId: id } })
}

/** Instances that launch with this profile by default (or last did). */
function usersOf(profile: string): DshInstance[] {
  if (!selectedHomeId.value) return []
  return instancesOfHome(selectedHomeId.value).filter(
    (i) => (i.default_profile ?? i.last_profile) === profile,
  )
}

// --- Profiles ------------------------------------------------------------------

const profiles = ref<string[]>([])
const profilesLoading = ref(false)
const newName = ref('')
const adding = ref(false)
const renaming = ref<string | null>(null)
const renameValue = ref('')
const copying = ref<string | null>(null)
const copyValue = ref('')

async function load() {
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

watch(selectedHomeId, () => void load(), { immediate: true })

const createAction = useAction((homeId: string, name: string) => api.createProfile(homeId, name), {
  success: (name) => t('profiles.profileCreated', { name }),
})

async function onCreate() {
  const name = newName.value.trim()
  if (!selectedHomeId.value || !name) return
  const created = await createAction.run(selectedHomeId.value, name)
  if (created === undefined) return
  newName.value = ''
  adding.value = false
  await load()
}

const renameAction = useAction(
  async (homeId: string, oldName: string, next: string) => {
    const renamed = await api.renameProfile(homeId, oldName, next)
    return { old: oldName, name: renamed }
  },
  { key: (_h, old) => old, success: ({ old, name }) => t('profiles.profileRenamed', { old, name }) },
)

function startRename(name: string) {
  renaming.value = name
  renameValue.value = name
}

async function confirmRename() {
  const old = renaming.value
  const next = renameValue.value.trim()
  if (!selectedHomeId.value || !old || !next || old === next) {
    renaming.value = null
    return
  }
  const res = await renameAction.run(selectedHomeId.value, old, next)
  if (res === undefined) return
  await store.refreshInstances()
  await load()
  renaming.value = null
}

const copyAction = useAction(
  async (homeId: string, source: string, name: string) => {
    const copied = await api.copyProfile(homeId, source, name)
    return { name: copied }
  },
  { success: ({ name }) => t('profiles.profileCopiedPlain', { name }) },
)

function startCopy(name: string) {
  copying.value = name
  copyValue.value = `${name}-copy`
}

async function confirmCopy() {
  const source = copying.value
  const name = copyValue.value.trim()
  if (!selectedHomeId.value || !source || !name) {
    copying.value = null
    return
  }
  const res = await copyAction.run(selectedHomeId.value, source, name)
  if (res === undefined) return
  await load()
  copying.value = null
}

const deleteAction = useAction(
  async (homeId: string, name: string) => {
    await api.deleteProfile(homeId, name)
    return name
  },
  { key: (_h, name) => name, success: (name) => t('profiles.profileDeleted', { name }) },
)

async function confirmDelete(name: string) {
  if (!selectedHomeId.value) return
  const deleted = await deleteAction.run(selectedHomeId.value, name)
  if (deleted === undefined) return
  await store.refreshInstances()
  await load()
}

const busy = (name: string) => !!renameAction.busy[name] || !!deleteAction.busy[name]
</script>

<template>
  <!-- The HOME axis is this pane's spine, and it is a list of records: a host
       that cannot be reached still has it. A failed read of the records is what
       this says, rather than showing an empty shelf that looks like "no HOMEs". -->
  <div v-if="store.homeError" class="dl-surface dl-failure">
    <p class="dl-failure-title">{{ t('library.hostErrorTitle') }}</p>
    <p class="dl-failure-text">{{ store.homeError }}</p>
    <p class="dl-failure-hint">{{ t('library.hostErrorHint') }}</p>
  </div>

  <div v-else-if="!selectedHomeId" class="dl-surface">
    <DlEmpty icon="profile" :description="t('profiles.noHomeSelected')" />
  </div>

  <template v-else>
    <!-- The HOME axis is a row of chips, not a select: there are usually two
         or three, and the chosen one's path stays legible beside them. It is a
         picker inside a section, not a page tab, so it deliberately does not
         wear the page-level segmented control (`.dl-segmented`). -->
    <div class="home-tabs">
      <button
        v-for="h in store.homes"
        :key="h.id"
        class="home-tab"
        :class="{ 'is-active': h.id === selectedHomeId }"
        @click="onSelectHome(h.id)"
      >
        {{ h.name }}
      </button>
      <span v-if="selectedHome" class="home-tab-path dl-mono" :title="selectedHome.path">
        {{ selectedHome.path }}
      </span>
    </div>

    <section class="dl-section">
      <div class="dl-section-label">
        {{ t('profiles.profilesTitle') }}
        <span class="dl-section-count">{{ profiles.length }}</span>
        <span class="profile-label-spacer" />
        <button v-if="!adding" class="dl-btn dl-btn--quiet" @click="adding = true">
          <DlIcon name="plus" :size="13" />
          {{ t('profiles.profileAdd') }}
        </button>
      </div>

      <div class="dl-surface profile-list">
        <div v-if="adding" class="profile-row is-editing">
          <input
            v-model="newName"
            class="dl-input"
            :placeholder="t('profiles.profileCreatePlaceholder')"
            @keydown.enter="onCreate"
          />
          <button class="dl-btn dl-btn--primary" :disabled="createAction.busy['*']" @click="onCreate">
            {{ t('profiles.profileCreate') }}
          </button>
          <button class="dl-btn dl-btn--quiet" @click="adding = false">{{ t('common.cancel') }}</button>
        </div>

        <DlEmpty
          v-if="!profilesLoading && profiles.length === 0 && !adding"
          icon="profile"
          :description="t('profiles.profilesEmpty')"
        />
        <div v-else-if="profilesLoading && profiles.length === 0" class="dl-row-none">
          {{ t('common.loading') }}
        </div>

        <template v-for="p in profiles" :key="p">
          <!-- Rename / copy turn the row into its own editor, so the row that
               is being changed is the row you type in. -->
          <div v-if="renaming === p" class="profile-row is-editing">
            <input v-model="renameValue" class="dl-input" @keydown.enter="confirmRename" />
            <button class="dl-btn dl-btn--primary" :disabled="renameAction.busy[p]" @click="confirmRename">
              {{ t('profiles.profileRenameSave') }}
            </button>
            <button class="dl-btn dl-btn--quiet" @click="renaming = null">{{ t('common.cancel') }}</button>
          </div>

          <div v-else-if="copying === p" class="profile-row is-editing">
            <span class="profile-copy-from dl-mono">{{ p }}</span>
            <DlIcon name="chevron-right" :size="13" class="profile-copy-arrow" />
            <input v-model="copyValue" class="dl-input" @keydown.enter="confirmCopy" />
            <button class="dl-btn dl-btn--primary" :disabled="copyAction.busy['*']" @click="confirmCopy">
              {{ t('profiles.profileCopySave') }}
            </button>
            <button class="dl-btn dl-btn--quiet" @click="copying = null">{{ t('common.cancel') }}</button>
          </div>

          <div v-else class="profile-row">
            <div class="profile-main">
              <div class="profile-title">
                <span class="profile-name dl-mono">{{ p }}</span>
              </div>
              <!-- Read-only on purpose: this row is about the profile, and the
                   instance's own default profile is an instance setting, edited
                   on the instance. Showing a select here made a profile row
                   read "web → web" and implied it did something to the profile. -->
              <div v-if="usersOf(p).length > 0" class="profile-users">
                <span class="profile-users-label">{{ t('profiles.usedByInstances') }}</span>
                <span v-for="inst in usersOf(p)" :key="inst.id" class="profile-user">{{ inst.name }}</span>
              </div>
              <span v-else class="profile-no-user">{{ t('profiles.noInstances') }}</span>
            </div>

            <div class="profile-actions">
              <button class="dl-btn dl-btn--ghost" :disabled="busy(p)" @click="startRename(p)">
                {{ t('profiles.profileRename') }}
              </button>
              <button class="dl-btn dl-btn--ghost" :disabled="busy(p)" @click="startCopy(p)">
                {{ t('profiles.profileCopy') }}
              </button>
              <a-popconfirm
                :content="t('profiles.profileDeleteConfirm', { name: p })"
                :ok-text="t('instances.table.delete')"
                :cancel-text="t('common.cancel')"
                modal-class="dl-sheet"
                @ok="confirmDelete(p)"
              >
                <button class="dl-btn dl-btn--danger" :disabled="busy(p)">
                  {{ t('instances.table.delete') }}
                </button>
              </a-popconfirm>
            </div>
          </div>
        </template>
      </div>

      <!-- A profile selects patch layers, but it does not own them: the running
           instance does. Saying so once, under the list, saves the user from
           hunting for a plugin button that was never here. -->
      <p class="profile-note">{{ t('profiles.pluginsLiveOnInstances') }}</p>
    </section>
  </template>
</template>

<style lang="scss" scoped>
.home-tabs {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.home-tab {
  padding: 4px 11px;
  border: 1px solid var(--dl-line);
  border-radius: 999px;
  background: var(--dl-surface);
  color: var(--dl-ink-2);
  font-family: inherit;
  font-size: var(--dl-text-sm);
  font-weight: 550;
  cursor: pointer;
  white-space: nowrap;
  transition:
    border-color var(--dl-fast) var(--dl-ease),
    background-color var(--dl-fast) var(--dl-ease),
    color var(--dl-fast) var(--dl-ease);

  &:hover:not(.is-active) {
    border-color: var(--dl-line-strong);
    color: var(--dl-ink);
  }

  &.is-active {
    border-color: var(--dl-signal-line);
    background: var(--dl-signal-soft);
    color: var(--dl-signal-ink);
  }
}

.home-tab-path {
  margin-left: 4px;
  min-width: 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-4);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dl-section-label {
  .profile-label-spacer {
    flex: 1;
  }
}

.profile-list {
  overflow: hidden;
}

.profile-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 13px 16px;

  & + .profile-row {
    border-top: 1px solid var(--dl-line);
  }

  &.is-editing {
    background: var(--dl-surface-2);

    .dl-input {
      flex: 1;
      max-width: 320px;
    }
  }
}

.profile-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.profile-name {
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.profile-users {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
}

.profile-user {
  display: inline-flex;
  align-items: center;
  padding: 1px 8px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface-2);
  font-family: var(--dl-font-mono);
  font-size: var(--dl-text-xs);
  color: var(--dl-ink-2);
}

.profile-users-label {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-4);
}

.profile-no-user {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-4);
}

.profile-note {
  margin: 10px 2px 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-4);
}

.profile-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.profile-copy-from {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-2);
}

.profile-copy-arrow {
  color: var(--dl-ink-4);
}
</style>

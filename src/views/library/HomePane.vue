<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useAction } from '@/composables/useAction'
import { useLauncherStore } from '@/stores/launcher'
import DlIcon from '@/components/DlIcon.vue'
import DlEmpty from '@/components/DlEmpty.vue'

// DSH_HOME directories the launcher knows about. A HOME is the data: profiles,
// sessions and credentials all live under it. Instances point at one, and a
// HOME that an instance points at cannot be forgotten — that is the only
// integrity rule this pane enforces.

const { t } = useI18n()
const router = useRouter()
const store = useLauncherStore()

const newName = ref('')
const newPath = ref('')

const usedBy = computed(() => (id: string) => store.instancesOfHome(id).length)

async function pickDir() {
  if (api.isTauri) {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog')
      const dir = await open({ directory: true, multiple: false })
      if (typeof dir === 'string') newPath.value = dir
    } catch (e) {
      Message.error(String(e))
    }
  } else {
    Message.info(t('homes.browserPickHint'))
  }
}

// Both actions name the host the pane is showing: a DSH_HOME belongs to one
// machine, and the record must never be filed under another (ADR-0006).
const addAction = useAction(
  (name: string, path: string) => api.createHome(name, path, store.currentHostId),
  { success: () => t('settings.saved') },
)

async function onAdd() {
  const home = await addAction.run(newName.value, newPath.value)
  if (home === undefined) return
  newName.value = ''
  newPath.value = ''
  await store.refreshHomes()
}

const removeAction = useAction(
  async (id: string) => {
    await api.removeHome(id, store.currentHostId)
    return true
  },
  { success: () => t('settings.saved') },
)

async function onRemove(id: string) {
  const ok = await removeAction.run(id)
  if (ok !== true) return
  await store.refreshHomes()
}

const dirAction = useAction((id: string) => api.openHomeDirectory(id), {
  key: (id) => id,
  success: (path) => t('homes.dirOpened', { path }),
})

function viewProfiles(homeId: string) {
  void router.push({ name: 'library', query: { tab: 'profiles', homeId } })
}
</script>

<template>
  <section class="dl-section">
    <div class="dl-section-label">
      {{ t('homes.homesTitle') }}
      <span class="dl-section-count">{{ store.homes.length }}</span>
    </div>

    <!-- Adding is an inline row, not a form above the list: the list is the
         page's subject and the new entry joins it in place. -->
    <div class="dl-surface home-add">
      <DlIcon name="folder" :size="15" class="home-add-glyph" />
      <input v-model="newName" class="dl-input" :placeholder="t('homes.homeNamePlaceholder')" />
      <input v-model="newPath" class="dl-input dl-input--mono home-path-input" :placeholder="t('homes.homePathPlaceholder')" />
      <button class="dl-btn dl-btn--quiet" @click="pickDir">{{ t('homes.pickDir') }}</button>
      <button
        class="dl-btn dl-btn--primary"
        :disabled="!newName.trim() || !newPath.trim() || addAction.busy['*']"
        @click="onAdd"
      >
        {{ t('homes.addHome') }}
      </button>
    </div>

    <!-- A DSH_HOME is a record the launcher keeps, so a machine that cannot be
         reached still has its list here; only a failed read of the records
         themselves replaces the list with what went wrong. -->
    <div v-if="store.homeError" class="dl-surface dl-failure">
      <p class="dl-failure-title">{{ t('library.hostErrorTitle') }}</p>
      <p class="dl-failure-text">{{ store.homeError }}</p>
      <p class="dl-failure-hint">{{ t('library.hostErrorHint') }}</p>
    </div>

    <div v-else class="dl-surface home-list">
      <DlEmpty v-if="store.homes.length === 0" icon="tray" :description="t('homes.homesEmpty')" />

      <div v-for="h in store.homes" :key="h.id" class="home-row">
        <div class="home-main">
          <div class="home-title">
            <span class="home-name">{{ h.name }}</span>
            <span v-if="usedBy(h.id) > 1" class="dl-tag dl-tag--warn">
              {{ t('homes.sharedTag') }}
            </span>
          </div>
          <span class="home-path dl-mono" :title="h.path">{{ h.path }}</span>
        </div>

        <span class="home-used dl-tnum">
          {{ usedBy(h.id) === 0 ? t('homes.unused') : t('homes.homeUsedByCount', { count: usedBy(h.id) }) }}
        </span>

        <div class="home-actions">
          <button
            class="dl-btn dl-btn--ghost"
            :disabled="!!dirAction.busy[h.id]"
            @click="dirAction.run(h.id)"
          >
            <DlIcon name="folder" :size="13" />
            {{ t('homes.openDir') }}
          </button>
          <button class="dl-btn dl-btn--ghost" @click="viewProfiles(h.id)">
            <DlIcon name="profile" :size="13" />
            {{ t('homes.viewProfiles') }}
          </button>
          <a-popconfirm
            :content="t('homes.confirmDeleteHome', { name: h.name })"
            :ok-text="t('homes.deleteHome')"
            :cancel-text="t('common.cancel')"
            :disabled="usedBy(h.id) > 0"
            modal-class="dl-sheet"
            @ok="onRemove(h.id)"
          >
            <button
              class="dl-btn dl-btn--danger"
              :disabled="usedBy(h.id) > 0"
              :title="usedBy(h.id) > 0 ? t('homes.deleteBlocked') : ''"
            >
              {{ t('homes.deleteHome') }}
            </button>
          </a-popconfirm>
        </div>
      </div>
    </div>
  </section>
</template>

<style lang="scss" scoped>
.home-add {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 16px;
  margin-bottom: 14px;

  .home-add-glyph {
    flex-shrink: 0;
    color: var(--dl-ink-3);
  }

  .home-path-input {
    flex: 1;
    min-width: 200px;
  }

  > .dl-input:first-of-type {
    width: 180px;
  }
}

.home-list {
  overflow: hidden;
}

.home-row {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 13px 16px;

  & + .home-row {
    border-top: 1px solid var(--dl-line);
  }
}

.home-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.home-title {
  display: flex;
  align-items: center;
  gap: 8px;
}

.home-name {
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.home-path {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.home-used {
  flex-shrink: 0;
  width: 96px;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.home-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}
</style>

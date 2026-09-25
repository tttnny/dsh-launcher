<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useAction } from '@/composables/useAction'
import { useLauncherStore } from '@/stores/launcher'
import DlEmpty from '@/components/DlEmpty.vue'

const { t } = useI18n()
const router = useRouter()
const store = useLauncherStore()

// --- HOME list ---------------------------------------------------------------

const newHomeName = ref('')
const newHomePath = ref('')

// Referential integrity comes from the store getters, not per-view filters.
const homeUsedBy = computed(() => (id: string) => store.instancesOfHome(id).length)
const instancesOfHome = store.instancesOfHome

async function onPickDir() {
  if (api.isTauri) {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog')
      const dir = await open({ directory: true, multiple: false })
      if (typeof dir === 'string') newHomePath.value = dir
    } catch (e) {
      Message.error(String(e))
    }
  } else {
    Message.info(t('homes.browserPickHint'))
  }
}

const addHomeAction = useAction((name: string, path: string) => api.createHome(name, path), {
  success: () => t('settings.saved'),
})

async function onAddHome() {
  const home = await addHomeAction.run(newHomeName.value, newHomePath.value)
  if (home === undefined) return
  newHomeName.value = ''
  newHomePath.value = ''
  await store.refreshHomes()
}

const removeHomeAction = useAction(async (id: string) => {
  await api.removeHome(id)
  return true
}, { success: () => t('settings.saved') })

async function onRemoveHome(id: string) {
  const ok = await removeHomeAction.run(id)
  if (ok !== true) return
  await store.refreshHomes()
}

function onViewProfiles(homeId: string) {
  void router.push({ path: '/profiles', query: { homeId } })
}

const dirAction = useAction((homeId: string) => api.openHomeDirectory(homeId), {
  key: (homeId) => homeId,
  success: (path) => t('homes.dirOpened', { path }),
})
const dirBusy = dirAction.busy
const onOpenHomeDir = dirAction.run

const homeColumns = computed(() => [
  { title: t('homes.homeName'), dataIndex: 'name', width: 170 },
  { title: t('homes.homePath'), dataIndex: 'path', ellipsis: true, tooltip: true },
  { title: t('homes.homeUsedBy'), slotName: 'usedBy', width: 140 },
  { title: t('instances.table.actions'), slotName: 'actions', width: 250, align: 'right' as const },
])
</script>

<template>
  <div class="dl-page homes-page">
    <!-- Home Management Card -->
    <div class="dl-card home-mgmt-card">
      <div class="dl-card-title">
        <h3>{{ t('homes.homesTitle') }}</h3>
      </div>
      <p class="dl-card-desc">{{ t('homes.homesDesc') }}</p>

      <div class="home-add-row">
        <input
          v-model="newHomeName"
          type="text"
          class="apple-input-sm"
          :placeholder="t('homes.homeNamePlaceholder')"
          style="width: 170px"
        />
        <div class="path-input-group">
          <input
            v-model="newHomePath"
            type="text"
            class="apple-input-sm path-input"
            :placeholder="t('homes.homePathPlaceholder')"
          />
          <button class="mac-secondary-btn" @click="onPickDir">
            {{ t('homes.pickDir') }}
          </button>
        </div>
        <button
          class="mac-primary-btn"
          :disabled="!newHomeName.trim() || !newHomePath.trim()"
          @click="onAddHome"
        >
          {{ t('homes.addHome') }}
        </button>
      </div>

      <a-table
        :columns="homeColumns"
        :data="store.homes"
        :pagination="false"
        row-key="id"
        class="apple-styled-table"
      >
        <template #usedBy="{ record }">
          <span class="home-used tnum">{{ t('homes.homeUsedByCount', { count: homeUsedBy(record.id) }) }}</span>
        </template>
        <template #actions="{ record }">
          <div class="action-cell-btns">
            <button
              class="mac-action-pill"
              :disabled="dirBusy[record.id]"
              @click="onOpenHomeDir(record.id)"
            >
              {{ t('homes.openDir') }}
            </button>
            <button
              class="mac-action-pill"
              @click="onViewProfiles(record.id)"
            >
              {{ t('homes.viewProfiles') }}
            </button>
            <a-popconfirm
              :content="t('homes.confirmDeleteHome', { name: record.name })"
              :disabled="homeUsedBy(record.id) > 0"
              @ok="onRemoveHome(record.id)"
            >
              <button
                class="mac-action-pill danger"
                :disabled="homeUsedBy(record.id) > 0"
              >
                {{ t('homes.deleteHome') }}
              </button>
            </a-popconfirm>
          </div>
        </template>
        <template #empty>
          <DlEmpty icon="tray" :description="t('homes.homesEmpty')" />
        </template>
      </a-table>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.homes-page {
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.home-add-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 16px;
  flex-wrap: wrap;

  .path-input-group {
    flex: 1;
    min-width: 260px;
    display: flex;
    align-items: center;
    gap: 6px;

    .path-input {
      flex: 1;
    }
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

.action-cell-btns {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  justify-content: flex-end;
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

  &.danger {
    color: rgb(var(--red-6));

    &:hover:not(:disabled) {
      background: rgb(var(--red-6) / 10%);
      border-color: rgb(var(--red-6) / 25%);
    }
  }

  &:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  &:active:not(:disabled) {
    transform: scale(var(--apple-active-scale));
  }
}

.home-used {
  color: var(--color-text-3);
  font-size: 12px;
}
</style>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import type { InstalledPlugin } from '@/api/types'
import { useAction } from '@/composables/useAction'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useLauncherStore()

// --- Selectors: HOME & Profile -----------------------------------------------

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

// --- Plugins of (selectedHomeId, selectedProfile) ----------------------------

const installedPlugins = ref<InstalledPlugin[]>([])
const pluginsLoading = ref(false)
const setEnabledBusy = ref(false)
const selectedPlugins = ref<string[]>([])

function displayVersion(raw?: string): string {
  if (!raw) return '-'
  return raw.replace(/^[\^~>=<\s]+/, '')
}

async function loadPlugins() {
  installedPlugins.value = []
  selectedPlugins.value = []
  if (!selectedHomeId.value || !selectedProfile.value) return
  pluginsLoading.value = true
  try {
    installedPlugins.value = await api.listInstalledPlugins(
      selectedHomeId.value,
      selectedProfile.value,
    )
  } catch (e) {
    Message.error(String(e))
  } finally {
    pluginsLoading.value = false
  }
}

watch([selectedHomeId, selectedProfile], () => {
  void loadPlugins()
})

async function onTogglePlugin(p: InstalledPlugin, enabled: boolean) {
  if (!selectedHomeId.value || !selectedProfile.value) return
  setEnabledBusy.value = true
  try {
    await api.setPluginsEnabled({
      homeId: selectedHomeId.value,
      profile: selectedProfile.value,
      pluginIds: [p.id],
      enabled,
    })
    p.enabled = enabled
    Message.success(
      enabled
        ? t('plugins.pluginEnabled', { name: p.id })
        : t('plugins.pluginDisabled', { name: p.id }),
    )
    Message.info(t('plugins.pluginRestartHint'))
  } catch (e) {
    Message.error(String(e))
    await loadPlugins()
  } finally {
    setEnabledBusy.value = false
  }
}

const uninstallAction = useAction(async (p: InstalledPlugin) => {
  await api.uninstallPlugin({
    homeId: selectedHomeId.value!,
    profile: selectedProfile.value!,
    pluginId: p.id,
  })
  return p.id
}, {
  success: (name) => t('plugins.pluginUninstalled', { name }),
})

async function onUninstallPlugin(p: InstalledPlugin) {
  if (!selectedHomeId.value || !selectedProfile.value) return
  const uninstalled = await uninstallAction.run(p)
  if (uninstalled === undefined) return
  Message.info(t('plugins.pluginRestartHint'))
  await loadPlugins()
}

// onTogglePlugin / batchSetEnabled stay manual: their catch re-syncs the list
// via loadPlugins(), which useAction's catch cannot express. This union keeps
// every plugin control disabled while any operation is in flight.
const pluginsBusy = computed(() => setEnabledBusy.value || uninstallAction.busy['*'])

async function batchSetEnabled(enabled: boolean) {
  if (
    !selectedHomeId.value ||
    !selectedProfile.value ||
    selectedPlugins.value.length === 0
  )
    return
  setEnabledBusy.value = true
  const ids = [...selectedPlugins.value]
  try {
    await api.setPluginsEnabled({
      homeId: selectedHomeId.value,
      profile: selectedProfile.value,
      pluginIds: ids,
      enabled,
    })
    for (const p of installedPlugins.value) {
      if (ids.includes(p.id)) p.enabled = enabled
    }
    selectedPlugins.value = []
    Message.success(
      enabled
        ? t('plugins.pluginsBatchEnabled', { count: ids.length })
        : t('plugins.pluginsBatchDisabled', { count: ids.length }),
    )
    Message.info(t('plugins.pluginRestartHint'))
  } catch (e) {
    Message.error(String(e))
    await loadPlugins()
  } finally {
    setEnabledBusy.value = false
  }
}

const pluginColumns = computed(() => [
  { title: 'ID', slotName: 'pid', width: 320 },
  { title: t('plugins.pluginVersion'), slotName: 'pver', width: 140 },
  { title: t('plugins.pluginStatus'), slotName: 'pstatus', width: 120 },
  { title: t('instances.table.actions'), slotName: 'pact', width: 90, align: 'right' as const },
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

    <!-- Plugins Table Card -->
    <div v-if="selectedHomeId && selectedProfile" class="dl-card plugins-card">
      <div class="dl-card-title">
        <div class="title-with-pill">
          <h3>{{ t('plugins.pluginsTitle') }}</h3>
          <span v-if="selectedHome" class="scope-pill">{{ selectedHome.name }} / {{ selectedProfile }}</span>
        </div>
      </div>
      <p class="dl-card-desc">{{ t('plugins.pluginsDesc') }}</p>

      <!-- Batch Actions Row -->
      <div v-if="installedPlugins.length > 0" class="plugins-batch-row">
        <div class="batch-btns">
          <button
            class="mac-action-pill"
            :disabled="selectedPlugins.length === 0 || pluginsBusy"
            @click="batchSetEnabled(true)"
          >
            {{ t('plugins.pluginsBatchEnable', { count: selectedPlugins.length }) }}
          </button>
          <button
            class="mac-action-pill"
            :disabled="selectedPlugins.length === 0 || pluginsBusy"
            @click="batchSetEnabled(false)"
          >
            {{ t('plugins.pluginsBatchDisable', { count: selectedPlugins.length }) }}
          </button>
        </div>
        <span v-if="selectedPlugins.length > 0" class="batch-count-hint">
          已选择 {{ selectedPlugins.length }} 个插件
        </span>
      </div>

      <a-table
        :columns="pluginColumns"
        :data="installedPlugins"
        :loading="pluginsLoading"
        :pagination="false"
        row-key="id"
        :row-selection="{
          type: 'checkbox',
          showCheckedAll: true,
          onlyCurrent: true,
          selectedRowKeys: selectedPlugins,
        }"
        class="apple-styled-table"
        @selection-change="(keys: (string | number)[]) => (selectedPlugins = keys.map(String))"
      >
        <template #pid="{ record }">
          <span class="plugin-cell-id tnum">{{ record.id }}</span>
        </template>
        <template #pver="{ record }">
          <span v-if="record.version" class="tnum">{{ displayVersion(record.version) }}</span>
          <span v-else class="plugin-no-version">-</span>
        </template>
        <template #pstatus="{ record }">
          <a-switch
            :model-value="record.enabled"
            :disabled="pluginsBusy"
            :checked-text="t('plugins.pluginOn')"
            :unchecked-text="t('plugins.pluginOff')"
            size="small"
            @change="(v: string | number | boolean) => onTogglePlugin(record, v === true)"
          />
        </template>
        <template #pact="{ record }">
          <a-popconfirm
            :content="t('plugins.pluginUninstallConfirm', { name: record.id })"
            @ok="onUninstallPlugin(record)"
          >
            <button class="mac-action-pill danger" :disabled="pluginsBusy">
              {{ t('instances.table.delete') }}
            </button>
          </a-popconfirm>
        </template>
        <template #empty>
          <a-empty :description="t('plugins.pluginsEmpty')" />
        </template>
      </a-table>
    </div>

    <!-- Empty Selection Card -->
    <div v-else class="dl-card">
      <a-empty :description="t('plugins.pluginsPickProfile')" />
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

.plugins-batch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;

  .batch-btns {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .batch-count-hint {
    font-size: 12px;
    color: var(--color-text-3);
  }
}

.plugin-cell-id {
  font-weight: 500;
  color: var(--color-text-1);
}

.plugin-no-version {
  color: var(--color-text-4);
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
</style>

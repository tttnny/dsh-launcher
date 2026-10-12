<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import type { PluginItem, PluginView } from '@/api/types'
import DlEmpty from '@/components/DlEmpty.vue'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useLauncherStore()

// --- Selectors: HOME & Profile -----------------------------------------------
// The selectors stay (HOME, Profile); the backend resolves them to the
// instance running that profile (design §4).

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

// --- The plugin view of (selectedHomeId, selectedProfile) ---------------------
// One snapshot from the running instance's native plugin manager. `mode`
// decides whether the page manages or only displays; `reason` is shown
// verbatim (the backend localizes it).

const pluginView = ref<PluginView | null>(null)
const pluginsLoading = ref(false)

const plugins = computed<PluginItem[]>(() => pluginView.value?.plugins ?? [])
const viewMode = computed(() => pluginView.value?.mode ?? 'not-running')
const viewReason = computed(() => pluginView.value?.reason ?? null)
const managed = computed(() => viewMode.value === 'managed')

/** Display name: the instance's localized title, falling back to the module. */
function pluginName(p: PluginItem): string {
  return p.title?.trim() || p.moduleName
}

async function loadPlugins() {
  pluginView.value = null
  if (!selectedHomeId.value || !selectedProfile.value) return
  pluginsLoading.value = true
  try {
    pluginView.value = await api.listPlugins(selectedHomeId.value, selectedProfile.value)
  } catch (e) {
    Message.error(String(e))
  } finally {
    pluginsLoading.value = false
  }
}

watch([selectedHomeId, selectedProfile], () => {
  void loadPlugins()
})

const pluginColumns = computed(() => [
  { title: t('plugins.pluginName'), slotName: 'pname' },
  { title: t('plugins.pluginEntry'), slotName: 'pentry', width: 320 },
  { title: t('plugins.pluginPhase'), slotName: 'pphase', width: 130 },
  { title: t('plugins.pluginStatus'), slotName: 'pstatus', width: 120 },
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

      <!-- Read-only banner: no instance running this profile, adopted instance,
           or an unreachable instance. The reason is backend-localized. -->
      <div v-if="!managed && viewReason" class="plugins-readonly-banner">
        {{ viewReason }}
      </div>

      <!-- The table renders whenever the instance gave us entries (managed), or
           a later ticket supplies an offline list; a pure read-only page shows
           only the banner, not a contradictory empty table. -->
      <a-table
        v-if="managed || plugins.length > 0"
        :columns="pluginColumns"
        :data="plugins"
        :loading="pluginsLoading"
        :pagination="false"
        row-key="entryId"
        class="apple-styled-table"
      >
        <template #pname="{ record }">
          <span class="plugin-cell-name">{{ pluginName(record) }}</span>
        </template>
        <template #pentry="{ record }">
          <span class="plugin-cell-id tnum">{{ record.entryId }}</span>
        </template>
        <template #pphase="{ record }">
          <span v-if="record.fiberPhase" class="plugin-phase">{{ record.fiberPhase }}</span>
          <span v-else class="plugin-no-version">-</span>
        </template>
        <template #pstatus="{ record }">
          <!-- The instance owns the state; the toggle is wired to the native
               writer by T2. Until then it reflects the instance's value. -->
          <a-switch
            :model-value="record.enabled"
            disabled
            :checked-text="t('plugins.pluginOn')"
            :unchecked-text="t('plugins.pluginOff')"
            size="small"
          />
        </template>
        <template #empty>
          <DlEmpty icon="plugin" :description="t('plugins.pluginsEmpty')" />
        </template>
      </a-table>
    </div>

    <!-- Empty Selection Card -->
    <div v-else class="dl-card">
      <DlEmpty icon="plugin" :description="t('plugins.pluginsPickProfile')" />
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

.plugins-readonly-banner {
  margin-bottom: 12px;
  padding: 8px 12px;
  border-radius: 6px;
  font-size: 13px;
  color: var(--color-text-2);
  background: var(--apple-group-bg);
  border: 1px solid var(--apple-card-border);
}

.plugin-cell-name {
  font-weight: 500;
  color: var(--color-text-1);
}

.plugin-cell-id {
  color: var(--color-text-3);
}

.plugin-phase {
  font-size: 12px;
  color: var(--color-text-3);
}

.plugin-no-version {
  color: var(--color-text-4);
}
</style>

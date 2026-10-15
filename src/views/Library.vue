<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useLauncherStore } from '@/stores/launcher'
import { useLibraryTab } from '@/composables/useLibraryTab'
import VersionPane from '@/views/library/VersionPane.vue'
import HomePane from '@/views/library/HomePane.vue'
import ProfilePane from '@/views/library/ProfilePane.vue'

// Library is one destination for the three things an instance can be built
// from: a DSH version, a DSH_HOME, and a Profile. They used to be three
// top-level pages of equal weight with the daily task. Here they are tabs of
// one page, and the tab is a real link (`#/library?tab=…`).
//
// 当前主机 sits next to the tabs because it is what the three tabs are *about*:
// every machine has its own versions, its own DSH_HOMEs and its own profiles,
// and installing a version means picking a machine. There is no 「全部」 here —
// a version is installed somewhere (ADR-0006 §3).

const { t } = useI18n()
const store = useLauncherStore()
const { current, select, tabs } = useLibraryTab()

const LABELS: Record<string, string> = {
  versions: 'library.tabVersions',
  homes: 'library.tabHomes',
  profiles: 'library.tabProfiles',
}

/** The one current host: the instances page's filter is the same state. */
const hostId = computed({
  get: () => store.currentHostId,
  set: (id: string) => void store.selectHost(id),
})
</script>

<template>
  <div class="dl-page">
    <header class="dl-head">
      <div class="dl-head-text">
        <h1>{{ t('library.title') }}</h1>
        <p class="dl-head-sub">{{ t('library.pageDesc') }}</p>
      </div>
      <div class="dl-head-actions">
        <div class="host-picker">
          <span class="host-picker-label">{{ t('library.currentHost') }}</span>
          <a-select v-model="hostId" size="small" class="host-picker-select">
            <a-option v-for="h in store.hosts" :key="h.id" :value="h.id">
              {{ h.display_name }}
            </a-option>
          </a-select>
        </div>
        <div class="dl-segmented">
          <button
            v-for="tab in tabs"
            :key="tab"
            :class="{ 'is-active': current === tab }"
            @click="select(tab)"
          >
            {{ t(LABELS[tab]) }}
          </button>
        </div>
      </div>
    </header>

    <VersionPane v-if="current === 'versions'" />
    <HomePane v-else-if="current === 'homes'" />
    <ProfilePane v-else />
  </div>
</template>

<style lang="scss" scoped>
// Only the label's type and the picker's width are local; the control itself is
// Arco's select, like the instance row's profile picker.
.host-picker {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-right: 12px;
}

.host-picker-label {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  white-space: nowrap;
}

.host-picker-select {
  width: 148px;
}
</style>

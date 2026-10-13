<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { useLibraryTab } from '@/composables/useLibraryTab'
import VersionPane from '@/views/library/VersionPane.vue'
import HomePane from '@/views/library/HomePane.vue'
import ProfilePane from '@/views/library/ProfilePane.vue'

// Library is one destination for the three things an instance can be built
// from: a DSH version, a DSH_HOME, and a Profile. They used to be three
// top-level pages of equal weight with the daily task. Here they are tabs of
// one page, and the tab is a real link (`#/library?tab=…`).

const { t } = useI18n()
const { current, select, tabs } = useLibraryTab()

const LABELS: Record<string, string> = {
  versions: 'library.tabVersions',
  homes: 'library.tabHomes',
  profiles: 'library.tabProfiles',
}
</script>

<template>
  <div class="dl-page">
    <header class="dl-head">
      <div class="dl-head-text">
        <h1>{{ t('library.title') }}</h1>
        <p class="dl-head-sub">{{ t('library.pageDesc') }}</p>
      </div>
      <div class="dl-head-actions">
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

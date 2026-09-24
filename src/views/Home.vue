<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { useLauncherStore } from '@/stores/launcher'
import { useProfiles } from '@/composables/useProfiles'
import HealthLogModal from '@/components/HealthLogModal.vue'
import InstanceCard from '@/components/InstanceCard.vue'

const router = useRouter()
const { t } = useI18n()
const store = useLauncherStore()
const { selectionByInstance: profileSel } = useProfiles()

// --- Card helpers -------------------------------------------------------------

const query = ref('')

const filteredInstances = computed(() => {
  const q = query.value.trim().toLowerCase()
  if (!q) return store.instances
  return store.instances.filter((i) => i.name.toLowerCase().includes(q))
})

const healthLogsVisible = ref(false)

</script>

<template>
  <div class="dl-page home-page">
    <!-- Apple-grade Toolbar -->
    <div class="home-toolbar">
      <div class="apple-search-wrapper">
        <svg class="search-icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8">
          <circle cx="7" cy="7" r="4.5" />
          <line x1="10.5" y1="10.5" x2="14" y2="14" stroke-linecap="round" />
        </svg>
        <input
          v-model="query"
          type="text"
          class="apple-search-input"
          :placeholder="t('home.searchPlaceholder')"
        />
        <button v-if="query" class="search-clear-btn" @click="query = ''">×</button>
      </div>

      <span class="home-count tnum">{{ t('home.instanceCount', { count: filteredInstances.length }) }}</span>

      <button
        class="mac-secondary-btn health-log-btn"
        :class="{
          'has-error': store.healthErrorCount > 0,
          'has-warn': store.healthErrorCount === 0 && store.healthWarnCount > 0
        }"
        @click="healthLogsVisible = true"
        :title="t('home.healthLogs')"
      >
        <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="3" width="12" height="10" rx="2" />
          <path d="M5 6.5l2 1.5-2 1.5" />
          <line x1="8.5" y1="9.5" x2="11" y2="9.5" />
        </svg>
        <span>{{ t('home.healthLogs') }}</span>
        <span
          v-if="store.healthTotalCount > 0"
          class="health-badge"
          :class="{
            'badge-error': store.healthErrorCount > 0,
            'badge-warn': store.healthErrorCount === 0 && store.healthWarnCount > 0
          }"
        >
          {{ store.healthTotalCount > 99 ? '99+' : store.healthTotalCount }}
        </span>
      </button>
    </div>

    <!-- Empty state -->
    <div v-if="store.instances.length === 0" class="dl-card home-empty">
      <div class="empty-icon-wrap">
        <svg viewBox="0 0 48 48" width="44" height="44" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <rect x="6" y="8" width="36" height="32" rx="4" />
          <line x1="6" y1="18" x2="42" y2="18" />
          <circle cx="12" cy="13" r="1.5" fill="currentColor" />
          <circle cx="17" cy="13" r="1.5" fill="currentColor" />
          <circle cx="22" cy="13" r="1.5" fill="currentColor" />
          <line x1="24" y1="26" x2="24" y2="34" />
          <line x1="20" y1="30" x2="28" y2="30" />
        </svg>
      </div>
      <div class="empty-title">{{ t('instances.emptyTitle') }}</div>
      <div class="empty-desc">{{ t('instances.emptyDesc') }}</div>
      <button class="mac-primary-btn" style="margin-top: 14px;" @click="router.push('/versions')">
        {{ t('versions.title') }}
      </button>
    </div>

    <div v-else-if="filteredInstances.length === 0" class="dl-card home-empty">
      <div class="empty-desc">{{ t('home.noMatchingInstances') }}</div>
    </div>

    <!-- Instance Card Wall -->
    <div v-else class="instance-grid">
      <InstanceCard v-for="inst in filteredInstances" :key="inst.id" :inst="inst" />
    </div>

    <HealthLogModal v-model:visible="healthLogsVisible" :profile-map="profileSel" />
  </div>
</template>

<style lang="scss" scoped>
.home-page {
  display: flex;
  flex-direction: column;
  gap: 20px;
}

// macOS Toolbar
.home-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
}

.apple-search-wrapper {
  position: relative;
  display: flex;
  align-items: center;
  width: 250px;

  .search-icon {
    position: absolute;
    left: 10px;
    color: var(--color-text-3);
    pointer-events: none;
  }

  .apple-search-input {
    width: 100%;
    height: 30px;
    padding: 0 28px 0 30px;
    font-size: 13px;
    border-radius: 8px;
    border: 1px solid var(--apple-card-border);
    background: var(--apple-card-bg);
    color: var(--color-text-1);
    outline: none;
    transition: all 0.16s ease;

    &:focus {
      border-color: rgb(var(--primary-6));
      box-shadow: 0 0 0 2px rgb(var(--primary-6) / 20%);
    }

    &::placeholder {
      color: var(--color-text-4);
    }
  }

  .search-clear-btn {
    position: absolute;
    right: 8px;
    border: none;
    background: transparent;
    color: var(--color-text-3);
    cursor: pointer;
    font-size: 14px;
    padding: 2px 4px;
    line-height: 1;

    &:hover {
      color: var(--color-text-1);
    }
  }
}

.home-count {
  font-size: 12px;
  color: var(--color-text-3);
  font-weight: 500;
}

.home-spacer {
  flex: 1;
}

// Apple Buttons
.mac-primary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 14px;
  font-size: 13px;
  font-weight: 500;
  border-radius: 8px;
  border: none;
  background: rgb(var(--primary-6));
  color: #fff;
  cursor: pointer;
  box-shadow: 0 1px 3px rgb(var(--primary-6) / 30%);
  transition: all 0.16s ease;

  &:hover {
    filter: brightness(1.06);
    box-shadow: 0 2px 8px rgb(var(--primary-6) / 45%);
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.mac-secondary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 12px;
  font-size: 13px;
  font-weight: 500;
  border-radius: 8px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  cursor: pointer;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  transition: all 0.16s ease;

  &:hover {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.health-log-btn {
  position: relative;

  &.has-error {
    border-color: rgba(239, 68, 68, 0.35);
    background: rgba(239, 68, 68, 0.06);
    color: #ef4444;

    &:hover {
      background: rgba(239, 68, 68, 0.12);
      color: #dc2626;
    }
  }

  &.has-warn {
    border-color: rgba(245, 158, 11, 0.35);
    background: rgba(245, 158, 11, 0.06);
    color: #f59e0b;

    &:hover {
      background: rgba(245, 158, 11, 0.12);
      color: #d97706;
    }
  }

  .health-badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 17px;
    height: 17px;
    padding: 0 4px;
    font-size: 10.5px;
    font-weight: 700;
    line-height: 1;
    border-radius: 9px;
    background: var(--apple-group-bg);
    color: var(--color-text-2);

    &.badge-error {
      background: #ef4444;
      color: #ffffff;
      box-shadow: 0 1px 3px rgba(239, 68, 68, 0.4);
    }

    &.badge-warn {
      background: #f59e0b;
      color: #ffffff;
      box-shadow: 0 1px 3px rgba(245, 158, 11, 0.4);
    }
  }
}

// Empty State
.home-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 56px 24px;
  text-align: center;
}

.empty-icon-wrap {
  color: var(--color-text-4);
  margin-bottom: 12px;
}

.empty-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--color-text-1);
  margin-bottom: 4px;
}

.empty-desc {
  font-size: 13px;
  color: var(--color-text-3);
  max-width: 380px;
}

// Card Wall Grid
.instance-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
  gap: 18px;
  align-items: stretch;
}
</style>

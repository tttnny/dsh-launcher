<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import DlEmpty from '@/components/DlEmpty.vue'
import type { RemoteVersion } from '@/api/types'

const router = useRouter()
const { t } = useI18n()
const store = useLauncherStore()

const loading = computed(() => store.remoteLoading)

onMounted(() => {
  store.refreshRemoteVersions()
})

const collator = new Intl.Collator('en', { numeric: true })

const sortedDesc = computed(() =>
  [...store.remoteVersions].sort((a, b) => collator.compare(b.version, a.version)),
)

const isPrerelease = (v: string) => v.includes('-')

const stable = computed(() => sortedDesc.value.filter((v) => !isPrerelease(v.version)))
const prerelease = computed(() => sortedDesc.value.filter((v) => isPrerelease(v.version)))

const latest = computed(() => {
  const rows: { v: RemoteVersion; label: string }[] = []
  if (stable.value[0]) rows.push({ v: stable.value[0], label: t('versions.latestStable') })
  if (prerelease.value[0]) rows.push({ v: prerelease.value[0], label: t('versions.latestPrerelease') })
  return rows
})

const installedSet = computed(() => new Set(store.versions.map((v) => v.version)))

function formatDate(iso: string | null): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

const installAction = useAction(
  async (version: string) => {
    await api.startInstallVersionTask(version)
    return version
  },
  {
    key: (version) => version,
    success: (version) => t('versions.installTaskStarted', { version }),
  },
)

function onSelectVersion(row: RemoteVersion) {
  if (installedSet.value.has(row.version)) {
    Message.info(t('versions.alreadyInstalled', { version: row.version }))
    return
  }
  const isSourceBuild = row.source === 'github'
  Modal.confirm({
    title: t('versions.confirmInstallTitle'),
    content: isSourceBuild
      ? t('versions.confirmInstallSourceContent', { version: row.version })
      : t('versions.confirmInstallContent', { version: row.version }),
    okText: t('versions.installNow'),
    cancelText: t('common.cancel'),
    onOk: async () => {
      const res = await installAction.run(row.version)
      if (res === undefined) return
      void router.push('/tasks')
    },
  })
}

const usedByCount = (versionId: string) => store.instancesOfVersion(versionId).length

const removeAction = useAction(
  async (id: string, version: string) => {
    await api.removeVersion(id)
    await store.refreshVersions()
    return version
  },
  {
    key: (id) => id,
    success: (version) => t('versions.versionDeleted', { version }),
  },
)
const onRemove = removeAction.run
</script>

<template>
  <div class="dl-page versions-page" v-loading="loading">
    <!-- Latest Card -->
    <div class="dl-card version-card">
      <div class="dl-card-title">
        <h3>{{ t('versions.latest') }}</h3>
        <div class="dl-toolbar">
          <button class="mac-secondary-btn" :disabled="loading" @click="store.refreshRemoteVersions()">
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8">
              <path d="M13.5 8A5.5 5.5 0 1 1 12 4.1L14 2" />
              <polyline points="14 5.5 14 2 10.5 2" />
            </svg>
            <span>{{ t('common.refresh') }}</span>
          </button>
        </div>
      </div>

      <template v-if="latest.length">
        <div
          v-for="row in latest"
          :key="row.v.version"
          class="version-row"
          @click="onSelectVersion(row.v)"
        >
          <div class="version-icon" :class="{ pre: isPrerelease(row.v.version) }">
            <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
              <line x1="16.5" y1="9.4" x2="7.5" y2="4.21" />
              <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
              <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
              <line x1="12" y1="22.08" x2="12" y2="12" />
            </svg>
          </div>
          <div class="version-meta">
            <div class="version-name">
              <span class="version-text tnum">{{ row.v.version }}</span>
              <span v-if="row.v.source === 'github'" class="source-build-chip">
                {{ t('versions.sourceBuildTag') }}
              </span>
              <span v-if="installedSet.has(row.v.version)" class="installed-chip">
                {{ t('versions.installedTag') }}
              </span>
            </div>
            <div class="version-sub">
              {{ row.label }}<template v-if="row.v.released_at">，{{ t('versions.releasedAt', { date: formatDate(row.v.released_at) }) }}</template>
            </div>
          </div>
          <div class="version-arrow-wrap">
            <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M6 12l4-4-4-4" />
            </svg>
          </div>
        </div>
      </template>
      <div v-else class="card-empty-text">{{ loading ? t('common.loading') : t('versions.noData') }}</div>
    </div>

    <!-- Disclosure Groups -->
    <a-collapse :default-active-key="['stable', 'prerelease']" class="apple-collapse-card">
      <a-collapse-item key="stable" :header="t('versions.stable')">
        <template v-if="stable.length">
          <div v-for="v in stable" :key="v.version" class="version-row" @click="onSelectVersion(v)">
            <div class="version-icon">
              <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <line x1="16.5" y1="9.4" x2="7.5" y2="4.21" />
                <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
                <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
                <line x1="12" y1="22.08" x2="12" y2="12" />
              </svg>
            </div>
            <div class="version-meta">
              <div class="version-name">
                <span class="version-text tnum">{{ v.version }}</span>
                <span v-if="v.source === 'github'" class="source-build-chip">
                  {{ t('versions.sourceBuildTag') }}
                </span>
                <span v-if="installedSet.has(v.version)" class="installed-chip">
                  {{ t('versions.installedTag') }}
                </span>
              </div>
              <div class="version-sub">{{ formatDate(v.released_at) }}</div>
            </div>
            <div class="version-arrow-wrap">
              <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M6 12l4-4-4-4" />
              </svg>
            </div>
          </div>
        </template>
        <div v-else class="card-empty-text">{{ t('versions.noData') }}</div>
      </a-collapse-item>

      <a-collapse-item key="prerelease" :header="t('versions.prerelease')">
        <template v-if="prerelease.length">
          <div v-for="v in prerelease" :key="v.version" class="version-row" @click="onSelectVersion(v)">
            <div class="version-icon pre">
              <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <line x1="16.5" y1="9.4" x2="7.5" y2="4.21" />
                <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
                <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
                <line x1="12" y1="22.08" x2="12" y2="12" />
              </svg>
            </div>
            <div class="version-meta">
              <div class="version-name">
                <span class="version-text tnum">{{ v.version }}</span>
                <span v-if="v.source === 'github'" class="source-build-chip">
                  {{ t('versions.sourceBuildTag') }}
                </span>
                <span v-if="installedSet.has(v.version)" class="installed-chip">
                  {{ t('versions.installedTag') }}
                </span>
              </div>
              <div class="version-sub">{{ formatDate(v.released_at) }}</div>
            </div>
            <div class="version-arrow-wrap">
              <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M6 12l4-4-4-4" />
              </svg>
            </div>
          </div>
        </template>
        <div v-else class="card-empty-text">{{ t('versions.noData') }}</div>
      </a-collapse-item>
    </a-collapse>

    <!-- Installed Card -->
    <div class="dl-card installed-card">
      <div class="dl-card-title">
        <h3>{{ t('versions.installedTitle') }}</h3>
      </div>
      <div v-for="v in store.versions" :key="v.id" class="installed-row">
        <div class="version-icon">
          <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="20 6 9 17 4 12" />
          </svg>
        </div>
        <div class="version-meta">
          <div class="version-name">
            <span class="version-text tnum">{{ v.version }}</span>
          </div>
          <div class="version-sub tnum">{{ v.dir }}</div>
        </div>
        <span class="used-by-pill tnum">{{ t('versions.usedBy', { count: usedByCount(v.id) }) }}</span>
        <a-popconfirm
          :content="t('versions.confirmDeleteVersion', { version: v.version })"
          @ok="onRemove(v.id, v.version)"
        >
          <button class="mac-action-pill danger">{{ t('versions.deleteVersion') }}</button>
        </a-popconfirm>
      </div>
      <DlEmpty v-if="store.versions.length === 0" icon="version" :description="t('versions.emptyInstalled')" />
    </div>
  </div>
</template>

<style lang="scss" scoped>
.versions-page {
  display: flex;
  flex-direction: column;
  gap: 18px;
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

  &:hover {
    filter: brightness(1.06);
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
  padding: 0 10px;
  font-size: 12.5px;
  font-weight: 500;
  border-radius: 7px;
  border: 1px solid var(--apple-card-border);
  background: var(--apple-card-bg);
  color: var(--color-text-2);
  cursor: pointer;
  transition: all 0.16s ease;

  &:hover:not(:disabled) {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  &:active:not(:disabled) {
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

  &.danger {
    color: rgb(var(--red-6));

    &:hover {
      background: rgb(var(--red-6) / 12%);
    }
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.version-row,
.installed-row {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 10px 12px;
  border-radius: 10px;
  cursor: pointer;
  transition: background 0.15s ease;

  &:hover {
    background: var(--apple-group-bg);
  }
}

.installed-row {
  cursor: default;
}

.version-icon {
  width: 36px;
  height: 36px;
  border-radius: 9px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: linear-gradient(135deg, #165dff, #722ed1);
  color: #fff;
  flex-shrink: 0;
  box-shadow: 0 2px 6px rgb(0 0 0 / 12%);

  &.pre {
    background: linear-gradient(135deg, #f7ba1e, #f53f3f);
  }
}

.version-meta {
  flex: 1;
  min-width: 0;
}

.version-name {
  display: flex;
  align-items: center;
  gap: 8px;

  .version-text {
    font-size: 14px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--color-text-1);
  }
}

.source-build-chip {
  padding: 1px 6px;
  font-size: 11px;
  font-weight: 600;
  border-radius: 5px;
  background: rgb(var(--orange-6) / 14%);
  color: rgb(var(--orange-6));
}

.installed-chip {
  padding: 1px 6px;
  font-size: 11px;
  font-weight: 600;
  border-radius: 5px;
  background: rgb(var(--green-6) / 14%);
  color: rgb(var(--green-6));
}

.version-sub {
  font-size: 12px;
  color: var(--color-text-3);
  margin-top: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.version-arrow-wrap {
  color: var(--color-text-4);
  display: flex;
  align-items: center;
}

.used-by-pill {
  padding: 2px 8px;
  font-size: 11.5px;
  border-radius: 6px;
  background: var(--apple-group-bg);
  color: var(--color-text-3);
  margin-right: 8px;
}

.apple-collapse-card {
  background: var(--apple-card-bg) !important;
  backdrop-filter: blur(24px);
  border: 1px solid var(--apple-card-border) !important;
  border-top: 1px solid var(--apple-card-border-top) !important;
  border-radius: var(--dl-card-radius) !important;
  overflow: hidden;
}

.card-empty-text {
  padding: 20px;
  text-align: center;
  color: var(--color-text-4);
  font-size: 13px;
}
</style>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import type { RemoteVersion, TaskInfo } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'
import DlEmpty from '@/components/DlEmpty.vue'
import DlRefreshButton from '@/components/DlRefreshButton.vue'

// Installing a version is the app's only long task, and its outcome is a
// version directory plus a 1:1 instance — so the progress lives on the version
// it installs, and there is no task page to visit.

const { t } = useI18n()
const store = useLauncherStore()

onMounted(() => {
  void store.refreshRemoteVersions()
  void store.refreshInstallTasks()
})

const collator = new Intl.Collator('en', { numeric: true })
const sortedDesc = computed(() =>
  [...store.remoteVersions].sort((a, b) => collator.compare(b.version, a.version)),
)
const isPrerelease = (v: string) => v.includes('-')

const installedSet = computed(() => new Set(store.versions.map((v) => v.version)))
const installedVersions = computed(() =>
  [...store.versions].sort((a, b) => collator.compare(b.version, a.version)),
)

/**
 * Installing a version names the machine it goes on, and an install only runs
 * where the launcher can drive npm and write the version directory — this Mac.
 * A remote host's tab still shows what that host could install; the button says
 * why it is not the one that does it.
 */
const canInstallHere = computed(() => store.currentHost?.builtin === true)

// Remote versions split into what can be installed and what already is.
const installable = computed(() => sortedDesc.value.filter((v) => !installedSet.value.has(v.version)))

type Channel = 'all' | 'stable' | 'prerelease'
const channel = ref<Channel>('all')
const search = ref('')

const visibleInstallable = computed(() => {
  const q = search.value.trim().toLowerCase()
  const inChannel = (() => {
    switch (channel.value) {
      case 'stable':
        return installable.value.filter((v) => !isPrerelease(v.version))
      case 'prerelease':
        return installable.value.filter((v) => isPrerelease(v.version))
      default:
        return installable.value
    }
  })()
  return q ? inChannel.filter((v) => v.version.toLowerCase().includes(q)) : inChannel
})

/** The search box is only worth its chrome once the list stops fitting. */
const searchable = computed(() => installable.value.length > 8)

const channelCounts = computed(() => ({
  all: installable.value.length,
  stable: installable.value.filter((v) => !isPrerelease(v.version)).length,
  prerelease: installable.value.filter((v) => isPrerelease(v.version)).length,
}))

function formatDate(iso: string | null): string {
  if (!iso) return ''
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleDateString()
}

// --- Install ------------------------------------------------------------------

const installAction = useAction(
  async (version: string) => {
    const taskId = await api.startInstallVersionTask(version)
    return { taskId, version }
  },
  { key: (version) => version, success: ({ version }) => t('versions.installTaskStarted', { version }) },
)

function installOf(version: string): TaskInfo | null {
  return store.installingVersions[version] ?? null
}

function onInstall(row: RemoteVersion) {
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
    modalClass: 'dl-sheet',
    onOk: async () => {
      const started = await installAction.run(row.version)
      if (started === undefined) return
      await store.watchInstallTask(started.taskId, row.version)
    },
  })
}

const cancelAction = useAction((id: string) => api.cancelTask(id), { key: (id) => id })

async function onCancelInstall(taskId: string) {
  await cancelAction.run(taskId)
  await store.refreshInstallTasks()
}

// --- Remove -------------------------------------------------------------------

const removeAction = useAction(
  async (id: string, version: string) => {
    await api.removeVersion(id, store.currentHostId)
    await store.refreshVersions()
    await store.refreshInstances()
    await store.refreshHomes()
    return version
  },
  { key: (id) => id, success: (version) => t('versions.versionDeleted', { version }) },
)

const usedByCount = (versionId: string) => store.instancesOfVersion(versionId).length
</script>

<template>
  <!-- Installed first: the versions you have are the ones you act on. A host
       that could not be reached has **no** list here — the previous one is not
       this host's answer (ADR-0006), so the section says what the machine said
       instead of showing it. -->
  <section class="dl-section">
    <div class="dl-section-label">
      {{ t('versions.installedTitle') }}
      <span class="dl-section-count">{{ installedVersions.length }}</span>
      <span class="ver-label-spacer" />
      <DlRefreshButton
        class="dl-icon-btn"
        :title="t('common.refresh')"
        :action="() => store.loadHostResources(store.currentHostId)"
      />
    </div>

    <div v-if="store.versionError" class="dl-surface dl-failure">
      <p class="dl-failure-title">{{ t('library.hostErrorTitle') }}</p>
      <p class="dl-failure-text">{{ store.versionError }}</p>
      <p class="dl-failure-hint">{{ t('library.hostErrorHint') }}</p>
    </div>

    <div v-else-if="installedVersions.length === 0" class="dl-surface">
      <DlEmpty icon="version" :description="t('versions.emptyInstalled')" />
    </div>

    <div v-else class="dl-surface">
      <div v-for="v in installedVersions" :key="v.id" class="ver-row">
        <span class="ver-mark"><DlIcon name="check" :size="14" /></span>
        <div class="ver-main">
          <div class="ver-title">
            <span class="ver-name dl-mono">{{ v.version }}</span>
            <span v-if="v.bin_ready" class="dl-tag dl-tag--run">{{ t('versions.installedTag') }}</span>
            <span v-else class="dl-tag dl-tag--warn" :title="t('versions.binMissingHint')">
              {{ t('versions.binMissing') }}
            </span>
          </div>
          <span class="ver-path dl-mono" :title="v.dir">{{ v.dir }}</span>
        </div>
        <span class="ver-used dl-tnum">{{ t('versions.usedBy', { count: usedByCount(v.id) }) }}</span>
        <a-popconfirm
          :content="t('versions.confirmDeleteVersion', { version: v.version, path: v.dedicated_home })"
          :ok-text="t('versions.deleteVersion')"
          :cancel-text="t('common.cancel')"
          modal-class="dl-sheet"
          @ok="removeAction.run(v.id, v.version)"
        >
          <button class="dl-btn dl-btn--danger" :disabled="!!removeAction.busy[v.id]">
            <DlIcon name="trash" :size="13" />
            {{ t('versions.deleteVersion') }}
          </button>
        </a-popconfirm>
      </div>
    </div>
  </section>

  <!-- Available: one filter row, then the versions that can still be installed.
       An install in flight renders its progress on its own row. -->
  <section class="dl-section">
    <div class="dl-section-label">
      {{ t('versions.availableTitle') }}
      <span class="dl-section-count">{{ channelCounts.all }}</span>
      <span class="ver-label-spacer" />
      <div v-if="searchable" class="dl-input dl-search ver-search">
        <DlIcon name="search" :size="13" />
        <input v-model="search" class="dl-search-input" :placeholder="t('versions.searchPlaceholder')" />
        <button v-if="search" class="dl-icon-btn dl-search-clear" @click="search = ''">
          <DlIcon name="close" :size="11" />
        </button>
      </div>
      <div class="dl-segmented ver-channels">
        <button :class="{ 'is-active': channel === 'all' }" @click="channel = 'all'">
          {{ t('versions.channelAll') }}
        </button>
        <button :class="{ 'is-active': channel === 'stable' }" @click="channel = 'stable'">
          {{ t('versions.stable') }}
        </button>
        <button :class="{ 'is-active': channel === 'prerelease' }" @click="channel = 'prerelease'">
          {{ t('versions.prerelease') }}
        </button>
      </div>
      <DlRefreshButton
        class="dl-icon-btn"
        :title="t('common.refresh')"
        :disabled="store.remoteLoading"
        :action="() => store.refreshRemoteVersions(true)"
      />
    </div>

    <div class="dl-surface">
      <!-- A failed query shows what the machine said, never the previous list:
           the installable set is a fact about that host's registry. -->
      <div v-if="store.remoteError" class="dl-row-none">{{ store.remoteError }}</div>

      <div v-else-if="visibleInstallable.length === 0" class="dl-row-none">
        {{
          store.remoteLoading
            ? t('common.loading')
            : search
              ? t('versions.searchEmpty')
              : t('versions.noData')
        }}
      </div>

      <!-- The list is that host's, but the install button is this Mac's: say so
           once, instead of leaving a row of greyed-out buttons unexplained. -->
      <div v-if="!canInstallHere && visibleInstallable.length > 0" class="dl-row-none">
        {{ t('versions.installLocalOnly') }}
      </div>

      <div v-for="v in visibleInstallable" :key="v.version" class="ver-row">
        <span class="ver-mark is-idle"><DlIcon name="download" :size="14" /></span>
        <div class="ver-main">
          <div class="ver-title">
            <span class="ver-name dl-mono">{{ v.version }}</span>
            <span v-if="isPrerelease(v.version)" class="dl-tag dl-tag--muted">
              {{ t('versions.prerelease') }}
            </span>
            <span v-if="v.source === 'github'" class="dl-tag dl-tag--warn">
              {{ t('versions.sourceBuildTag') }}
            </span>
          </div>
          <span class="ver-sub">
            {{ t('versions.releasedAt', { date: formatDate(v.released_at) }) }}
          </span>

          <div v-if="installOf(v.version)" class="ver-progress">
            <a-progress
              :percent="(installOf(v.version)?.percent ?? 0) / 100"
              size="small"
              :show-text="true"
            />
            <button
              class="dl-btn dl-btn--quiet"
              :disabled="!!cancelAction.busy[installOf(v.version)?.id ?? '']"
              @click="onCancelInstall(installOf(v.version)!.id)"
            >
              {{ t('versions.cancelInstall') }}
            </button>
          </div>
        </div>

        <button
          v-if="!installOf(v.version)"
          class="dl-btn dl-btn--quiet ver-install"
          :disabled="!!installAction.busy[v.version] || !canInstallHere"
          :title="canInstallHere ? undefined : t('versions.installLocalOnly')"
          @click="onInstall(v)"
        >
          {{ t('versions.installNow') }}
        </button>
        <span v-else class="dl-tag dl-tag--signal ver-installing">
          {{ t('versions.installing') }}
        </span>
      </div>
    </div>
  </section>
</template>

<style lang="scss" scoped>
.dl-section-label {
  .ver-label-spacer {
    flex: 1;
  }
}

.ver-channels {
  margin-right: 4px;
}

.ver-search {
  width: 190px;
  height: 28px;
  margin-right: 8px;
}

.ver-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;

  & + .ver-row {
    border-top: 1px solid var(--dl-line);
  }
}

.ver-mark {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  flex-shrink: 0;
  border-radius: var(--dl-r-ctl);
  background: var(--dl-run-soft);
  color: var(--dl-run);

  &.is-idle {
    background: var(--dl-surface-3);
    color: var(--dl-ink-3);
  }
}

.ver-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.ver-title {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ver-name {
  font-size: var(--dl-text-lg);
  font-weight: 600;
  color: var(--dl-ink);
}

.ver-path,
.ver-sub {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ver-used {
  flex-shrink: 0;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.ver-install {
  flex-shrink: 0;
}

.ver-installing {
  flex-shrink: 0;
}

.ver-progress {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 6px;
  max-width: 460px;

  :deep(.arco-progress) {
    flex: 1;
    margin: 0;
  }
}

</style>

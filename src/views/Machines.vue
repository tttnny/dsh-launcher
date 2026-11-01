<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { api } from '@/api'
import type { HostWithStatus } from '@/api/types'
import { useLauncherStore } from '@/stores/launcher'
import PageHeader from '@/components/PageHeader.vue'
import EmptyState from '@/components/EmptyState.vue'
import MachineFormDialog from '@/components/MachineFormDialog.vue'
import DlIcon from '@/components/DlIcon.vue'

// 机器列表：每行一台机器，点进去看连接、环境和资源。

const router = useRouter()
const store = useLauncherStore()

const loading = ref(!store.loaded)
const loadError = ref('')
const formVisible = ref(false)
const editingHost = ref<HostWithStatus | null>(null)

async function loadHosts() {
  if (!store.loaded) loading.value = true
  try {
    const list = await api.listHosts()
    loadError.value = ''
    store.setHosts(list)
    for (const host of list) {
      const missingVersions =
        !store.versionsByHost[host.id] && !store.versionErrors[host.id]
      const missingHomes =
        !store.homesByHost[host.id] && !store.homeErrors[host.id]
      if (missingVersions || missingHomes) {
        void store.loadHostResources(host.id)
      }
    }
  } catch (e) {
    loadError.value = String(e)
  } finally {
    loading.value = false
  }
}

onMounted(loadHosts)

function badgeState(host: HostWithStatus): string {
  if (host.builtin) return 'ok'
  const state = host.status?.state
  if (state === 'reachable') return 'ok'
  if (state === 'unreachable') return 'unreachable'
  if (state === 'connecting') return 'starting'
  return 'idle'
}

function addHost() {
  editingHost.value = null
  formVisible.value = true
}

function openDetail(host: HostWithStatus) {
  void router.push({ name: 'machine-detail', params: { id: host.id } })
}
</script>

<template>
  <div class="page">
    <PageHeader
      title="机器"
      description="每台机器有自己独立的版本、数据目录和运行环境。"
    >
      <template #actions>
        <button class="btn btn--primary" @click="addHost">
          <DlIcon name="plus" :size="13" />
          添加主机
        </button>
      </template>
    </PageHeader>

    <div v-if="loadError" class="notice notice--danger list-notice">
      <div class="notice-body">
        <div class="notice-title">读不到机器列表</div>
        <div>{{ loadError }}</div>
      </div>
      <button class="btn btn--quiet btn--sm" @click="loadHosts">
        重新读取
      </button>
    </div>

    <div
      v-if="loading && store.hosts.length === 0"
      class="surface surface--pad reading"
    >
      <span class="spinner" />
      正在读取机器…
    </div>

    <div
      v-else-if="!loading && !loadError && store.hosts.length === 0"
      class="surface"
    >
      <EmptyState
        icon="machine"
        title="还没有机器"
        description="添加一台远程主机，或直接使用内置的本机。"
      >
        <button class="btn btn--primary" @click="addHost">添加主机</button>
      </EmptyState>
    </div>

    <div v-else class="surface machine-list">
      <div
        v-for="host in store.hosts"
        :key="host.id"
        class="list-row machine-row"
        @click="openDetail(host)"
      >
        <div class="row-main">
          <div class="row-title">
            <span class="host-dot" :class="'is-' + badgeState(host)" />
            <span>{{ host.display_name }}</span>
            <span class="tag tag--muted">{{
              host.builtin ? '内置' : '远程'
            }}</span>
            <span
              v-if="!host.builtin && host.status?.state === 'unreachable'"
              class="tag tag--warn"
              >不可达</span
            >
          </div>
          <div class="row-meta">
            <span
              v-if="host.ssh_target"
              class="mono"
              :title="host.ssh_target"
              >{{ host.ssh_target }}</span
            >
            <span v-else>这台 Mac</span>
          </div>
        </div>
        <DlIcon name="chevron-right" :size="14" class="row-chev" />
      </div>
    </div>

    <MachineFormDialog
      v-model:visible="formVisible"
      :host="editingHost"
      @saved="loadHosts"
    />
  </div>
</template>

<style lang="scss" scoped>
.list-notice {
  margin-bottom: 14px;
  align-items: center;
}

.reading {
  display: flex;
  align-items: center;
  gap: 9px;
  color: var(--dl-ink-3);
}

.machine-list {
  overflow: hidden;
}

.host-dot {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--dl-ink-4);

  &.is-ok {
    background: var(--dl-run);
  }

  &.is-unreachable,
  &.is-starting {
    background: var(--dl-warn);
  }
}

.row-chev {
  flex-shrink: 0;
  margin-left: -4px;
  color: var(--dl-ink-4);
}

.machine-row {
  cursor: pointer;

  &:hover {
    background: var(--dl-surface-2);
  }
}

.meta-sep {
  color: var(--dl-ink-4);
}

.row-reason {
  max-width: 74ch;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--dl-warn);
  font-size: var(--dl-text-xs);
}

.test-head {
  display: flex;
  align-items: center;
  gap: 9px;
  margin-bottom: 10px;
}

.test-host {
  font-weight: 600;
  color: var(--dl-ink);
}

.test-message {
  margin: 0 0 10px;
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
  line-height: 1.6;
}

.test-facts {
  display: grid;
  gap: 8px;
  padding: 12px 14px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-card);
  background: var(--dl-surface-2);
}

.test-fact {
  display: flex;
  align-items: baseline;
  gap: 12px;
  min-width: 0;
}

.test-label {
  width: 62px;
  flex-shrink: 0;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.test-value {
  min-width: 0;
  overflow-wrap: anywhere;
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
}
</style>

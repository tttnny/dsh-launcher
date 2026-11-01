<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useLauncherStore } from '@/stores/launcher'
import DlIcon from '@/components/DlIcon.vue'
import launcherIcon from '@/assets/launcher-icon.png'

// 左侧导航。五个页面，一层，没有第二组机器列表：机器只属于「机器」页。

const store = useLauncherStore()
const route = useRoute()
const router = useRouter()

type NavKey = 'instances' | 'install' | 'data' | 'machines' | 'settings'

const navItems = computed<
  {
    key: NavKey
    label: string
    icon: 'instances' | 'download' | 'database' | 'host'
  }[]
>(() => [
  { key: 'instances', label: '实例', icon: 'instances' },
  { key: 'install', label: '安装', icon: 'download' },
  { key: 'data', label: '数据', icon: 'database' },
  { key: 'machines', label: '机器', icon: 'host' },
])

const runningCount = computed(
  () =>
    store.instances.filter((i) => store.statusOf(i.id).state === 'running')
      .length,
)

function activeKey(): NavKey {
  const name = String(route.name ?? '')
  if (name === 'install') return 'install'
  if (name === 'data') return 'data'
  if (name === 'machines' || name === 'machine-detail') return 'machines'
  if (name === 'settings') return 'settings'
  return 'instances'
}

function go(path: string) {
  if (route.path === path) return
  void router.push(path).catch(() => undefined)
}
</script>

<template>
  <aside class="rail">
    <div class="rail-drag" data-tauri-drag-region />

    <div class="rail-brand">
      <img class="rail-mark" :src="launcherIcon" alt="" />
      <span class="rail-name">DSH Launcher</span>
    </div>

    <nav class="rail-nav" aria-label="主导航">
      <button
        v-for="item in navItems"
        :key="item.key"
        class="rail-item"
        :class="{ 'is-active': activeKey() === item.key }"
        @click="go('/' + (item.key === 'instances' ? '' : item.key))"
      >
        <DlIcon :name="item.icon" :size="16" />
        <span class="rail-label">{{ item.label }}</span>
        <span
          v-if="item.key === 'instances' && runningCount > 0"
          class="rail-count"
          >{{ runningCount }}</span
        >
      </button>
    </nav>

    <div class="rail-grow" />

    <button
      class="rail-item rail-settings"
      :class="{ 'is-active': activeKey() === 'settings' }"
      @click="go('/settings')"
    >
      <DlIcon name="settings" :size="16" />
      <span class="rail-label">设置</span>
    </button>
  </aside>
</template>

<style lang="scss" scoped>
.rail {
  display: flex;
  flex-direction: column;
  width: var(--dl-rail-w);
  flex-shrink: 0;
  height: 100%;
  padding: 0 10px 12px;
  border-right: 1px solid var(--dl-rail-line);
  background: var(--dl-rail);
  color: var(--dl-rail-ink);
  backdrop-filter: saturate(180%) blur(20px);
  -webkit-backdrop-filter: saturate(180%) blur(20px);
  user-select: none;
}

.rail-drag {
  height: 44px;
  flex-shrink: 0;
}

.rail-brand {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 6px 10px 22px;
}

.rail-mark {
  width: 26px;
  height: 26px;
  border-radius: 7px;
  flex-shrink: 0;
}

.rail-name {
  color: var(--dl-rail-ink);
  font-size: 14px;
  font-weight: 600;
  letter-spacing: -0.01em;
  white-space: nowrap;
}

.rail-nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.rail-grow {
  flex: 1;
}

.rail-item {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  height: 32px;
  padding: 0 10px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--dl-rail-ink);
  font-family: inherit;
  font-size: var(--dl-text-md);
  font-weight: 400;
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--dl-fast) var(--dl-ease),
    color var(--dl-fast) var(--dl-ease);

  &:hover {
    background: var(--dl-rail-hover);
  }

  &.is-active {
    background: var(--dl-rail-active);
    font-weight: 500;

    :deep(svg) {
      color: var(--dl-accent);
    }
  }
}

.rail-label {
  flex: 1;
  min-width: 0;
}

.rail-count {
  min-width: 16px;
  color: var(--dl-rail-dim);
  font-size: var(--dl-text-xs);
  font-weight: 500;
  text-align: right;
}

.rail-settings {
  margin-top: 8px;
}
</style>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { useLauncherStore } from '@/stores/launcher'
import type { ThemeMode } from '@/api/types'
import { api } from '@/api'
import { Message } from '@arco-design/web-vue'
import {
  MENU_NAVIGATE_EVENTS,
  MENU_REFRESH_EVENTS,
  handleAppKeydown,
  resolveMenuRoute,
  type ShortcutRoute,
} from '@/shortcuts'

const route = useRoute()
const router = useRouter()
const { t, locale } = useI18n()
const store = useLauncherStore()
const isTauri = api.isTauri

// --- Theme: light / dark / follow system -------------------------------------

const themeMedia = window.matchMedia('(prefers-color-scheme: dark)')

/** Applies the effective Arco theme to <body> (body[arco-theme='dark']). */
function applyTheme(mode: ThemeMode) {
  const dark = mode === 'dark' || (mode === 'system' && themeMedia.matches)
  if (dark) {
    document.body.setAttribute('arco-theme', 'dark')
  } else {
    document.body.removeAttribute('arco-theme')
  }
}

function onSystemThemeChange() {
  if (store.settings.theme === 'system') applyTheme('system')
}

onMounted(async () => {
  await store.init()
  locale.value = store.settings.locale || 'zh-CN'
  applyTheme(store.settings.theme || 'system')
  themeMedia.addEventListener('change', onSystemThemeChange)
  // Land on the environment check when the toolchain is incomplete. pnpm is
  // included because it is a hard requirement for installing a version and for
  // plugin management, and the launcher no longer installs one itself — being
  // sent here is the only place the user learns that. Navigation stays open
  // (the sidebar is untouched) so a network-restricted user can still reach
  // Settings and configure a proxy.
  const runtimeIncomplete =
    !store.runtime?.node?.installed || !store.runtime?.pnpm?.installed
  if (runtimeIncomplete && route.name !== 'setup') {
    router.push({ name: 'setup' })
  }
  await setupLaunchDeepLink()
  window.addEventListener('keydown', onAppKeydown)
  await setupMenuListeners()
})

// --- In-app shortcuts: Cmd/Ctrl+1/2/3/4, Cmd+, K/R, Esc -----------------------

function goShortcut(name: ShortcutRoute) {
  if (name === 'instances' && route.name === 'instance-edit') return
  if (route.name === name) return
  void router.push({ name }).catch(() => undefined)
}

function refreshShortcut() {
  void Promise.allSettled([store.refreshInstances(), store.refreshTasks(), store.checkRuntime()])
}

function backShortcut() {
  if (route.name === 'home') return
  router.back()
}

function onAppKeydown(e: KeyboardEvent) {
  if (handleAppKeydown(e, { go: goShortcut, refresh: refreshShortcut, back: backShortcut })) {
    e.preventDefault()
  }
}

let unlistenMenu: (() => void)[] = []

/** Backend menu events (Rust menu) drive the same routes as shortcuts. */
async function setupMenuListeners() {
  if (!isTauri) return
  try {
    const { listen } = await import('@tauri-apps/api/event')
    for (const name of MENU_NAVIGATE_EVENTS) {
      const un = await listen<unknown>(name, (event) => {
        const target = resolveMenuRoute(event.payload)
        if (target) goShortcut(target)
      })
      unlistenMenu.push(un)
    }
    for (const name of MENU_REFRESH_EVENTS) {
      const un = await listen(name, () => refreshShortcut())
      unlistenMenu.push(un)
    }
    unlistenMenu.push(await listen('check-update', () => void onMenuCheckUpdate()))
    unlistenMenu.push(await listen('open-help', () => void onMenuOpenHelp()))
  } catch {
    // Event bridge unavailable (browser preview); shortcuts still work.
  }
}

let checkingMenuUpdate = false

async function onMenuCheckUpdate() {
  if (checkingMenuUpdate) return
  checkingMenuUpdate = true
  try {
    const info = await api.checkLauncherUpdate('dev')
    if (info.up_to_date) {
      Message.success(t('settings.update.upToDate'))
    } else {
      Message.info(t('settings.update.available', { version: info.latest ?? '' }))
      if (info.url) await api.openExternal(info.url)
    }
  } catch (e) {
    Message.error(String(e))
  } finally {
    checkingMenuUpdate = false
  }
}

async function onMenuOpenHelp() {
  try {
    await api.openExternal('https://github.com/dsh-plugins/dsh-launcher')
  } catch (e) {
    Message.error(String(e))
  }
}

// --- Launch deep link: dsh-launcher://launch → start instance ---------

let unlistenDeepLink: (() => void) | undefined

async function setupLaunchDeepLink() {
  if (!isTauri) return
  const pending = await api.pendingDeepLink()
  if (pending) handleDeepLink(pending)
  const { listen } = await import('@tauri-apps/api/event')
  unlistenDeepLink = await listen<string>('deep-link', (event) => handleDeepLink(event.payload))
}

function handleDeepLink(raw: string) {
  try {
    const u = new URL(raw)
    if (u.protocol !== 'dsh-launcher:') return
    if (u.host === 'launch') {
      void launchFromDeepLink(u)
    }
  } catch {
    // Ignore invalid protocol
  }
}

async function launchFromDeepLink(u: URL) {
  const ref = u.searchParams.get('instance')
  if (!ref) return
  const inst = store.instances.find((i) => i.id === ref || i.name === ref)
  if (!inst) {
    Message.error(String(ref))
    return
  }
  try {
    const state = store.statusOf(inst.id).state
    if (state !== 'running' && state !== 'starting') {
      const profile = u.searchParams.get('profile') || inst.default_profile || 'web'
      await api.startInstance(inst.id, profile)
    }
    await openBrowserWhenReady(inst.id)
  } catch (e) {
    Message.error(String(e))
  }
}

async function openBrowserWhenReady(id: string) {
  const deadline = Date.now() + 120_000
  for (;;) {
    const st = store.statusOf(id)
    if (st.state === 'running' && st.url) {
      await api.openInstanceWindow(id)
      return
    }
    if (st.state === 'exited' || Date.now() > deadline) {
      await api.openInstanceWindow(id)
      return
    }
    await new Promise((r) => setTimeout(r, 500))
  }
}

onUnmounted(() => {
  themeMedia.removeEventListener('change', onSystemThemeChange)
  window.removeEventListener('keydown', onAppKeydown)
  unlistenDeepLink?.()
  unlistenMenu.forEach((un) => un())
  unlistenMenu = []
})

watch(
  () => store.settings.locale,
  (v) => {
    if (v) locale.value = v
  },
)

watch(
  () => store.settings.theme,
  (v) => {
    if (v) applyTheme(v)
  },
)

// --- Sidebar navigation -------------------------------------------------------

type NavKey = 'home' | 'homes' | 'profiles' | 'plugins' | 'versions' | 'tasks' | 'settings'

const siderCollapsed = ref(localStorage.getItem('dsh-launcher.siderCollapsed') === '1')

function toggleSider() {
  siderCollapsed.value = !siderCollapsed.value
  try {
    localStorage.setItem('dsh-launcher.siderCollapsed', siderCollapsed.value ? '1' : '0')
  } catch {
    // Private mode fallback
  }
}

const navSelected = computed<NavKey>(() => {
  const name = route.name as string
  if (name === 'instance-edit') return 'home'
  if (name === 'homes') return 'homes'
  if (name === 'profiles') return 'profiles'
  if (name === 'plugins') return 'plugins'
  if (name === 'versions') return 'versions'
  if (name === 'tasks') return 'tasks'
  if (name === 'settings') return 'settings'
  return 'home'
})

function navGo(key: NavKey) {
  switch (key) {
    case 'home':
      void router.push({ name: 'home' }).catch(() => undefined)
      break
    case 'homes':
      void router.push({ name: 'homes' }).catch(() => undefined)
      break
    case 'profiles':
      void router.push({ name: 'profiles' }).catch(() => undefined)
      break
    case 'plugins':
      void router.push({ name: 'plugins' }).catch(() => undefined)
      break
    case 'versions':
      void router.push({ name: 'versions' }).catch(() => undefined)
      break
    case 'tasks':
      void router.push({ name: 'tasks' }).catch(() => undefined)
      break
    case 'settings':
      void router.push({ name: 'settings' }).catch(() => undefined)
      break
  }
}

const runningInstanceCount = computed(
  () => store.instances.filter((i) => store.statusOf(i.id).state === 'running').length,
)

const pageTitle = computed(() => {
  const name = route.name as string
  switch (name) {
    case 'home':
      return t('nav.home')
    case 'instances':
      return t('instances.title')
    case 'instance-edit':
      return t('instanceEdit.titleEdit')
    case 'homes':
      return t('homes.title')
    case 'profiles':
      return t('profiles.title')
    case 'plugins':
      return t('plugins.title')
    case 'versions':
      return t('versions.title')
    case 'tasks':
      return t('tasks.title')
    case 'settings':
      return t('settings.title')
    case 'setup':
      return t('setup.title')
    default:
      return t('app.title')
  }
})

const showBack = computed(() => {
  const name = route.name as string
  return name === 'instance-edit'
})

function onHeaderBack() {
  router.back()
}

const appWindow = (() => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (window as any)?.__TAURI_INTERNALS__ ? loadWindowApi() : null
})()

async function loadWindowApi() {
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  return getCurrentWindow()
}

// --- Title bar drag & double-click zoom ---------------------------------------
// startDragging() enters a native drag loop that swallows the pending click,
// so only start dragging once the pointer moves past a small threshold — fast
// clicks then complete normally and dblclick (zoom) still reaches the webview.

let dragArmed: { x: number; y: number; started: boolean } | null = null

function onHeaderMouseDown(e: MouseEvent) {
  if (!isTauri || e.button !== 0) return
  const el = e.target as HTMLElement | null
  if (el?.closest('.nav-item, a, button, input, [data-no-drag]')) return
  dragArmed = { x: e.screenX, y: e.screenY, started: false }
  window.addEventListener('mousemove', onDragThresholdMove)
  window.addEventListener('mouseup', onDragFinish, { once: true })
}

function onDragThresholdMove(e: MouseEvent) {
  if (!dragArmed || dragArmed.started) return
  const dx = e.screenX - dragArmed.x
  const dy = e.screenY - dragArmed.y
  if (dx * dx + dy * dy > 25) {
    dragArmed.started = true
    void Promise.resolve(appWindow).then((w) => w?.startDragging())
  }
}

function onDragFinish() {
  dragArmed = null
  window.removeEventListener('mousemove', onDragThresholdMove)
}

/** macOS native title bar behavior: double-click empty strip zooms the window. */
async function onHeaderDoubleClick(e: MouseEvent) {
  if (!isTauri || e.button !== 0) return
  const el = e.target as HTMLElement | null
  if (el?.closest('.nav-item, a, button, input, [data-no-drag]')) return
  await toggleWindowZoom()
}

// --- Window zoom: the native traffic lights own close / minimize / zoom, but
// double-clicking the custom header strip still zooms the window. ---

async function toggleWindowZoom() {
  try {
    const w = await appWindow
    if (!w) return
    if (await w.isMaximized()) {
      await w.unmaximize()
    } else {
      await w.maximize()
    }
  } catch {
    // Advisory only
  }
}
</script>

<template>
  <div class="apple-window">
    <!-- Unified Sidebar: spans full vertical height -->
    <aside class="apple-sider" :class="{ collapsed: siderCollapsed }">
      <!-- Sidebar Header: a bare drag strip. The native traffic lights draw
           over its left end, exactly as they do over Finder's sidebar; the
           sidebar toggle lives in the main toolbar (see .header-left). -->
      <div
        class="sider-traffic-header"
        @mousedown="onHeaderMouseDown"
        @dblclick="onHeaderDoubleClick"
      />

      <!-- Navigation List -->
      <nav class="sider-nav">
        <button
          class="nav-item"
          :class="{ active: navSelected === 'home' }"
          :title="siderCollapsed ? t('nav.home') : ''"
          @click="navGo('home')"
        >
          <span class="nav-icon">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <polygon points="5 3 19 12 5 21 5 3" />
            </svg>
          </span>
          <span v-if="!siderCollapsed" class="nav-label">{{ t('nav.home') }}</span>
          <span v-if="!siderCollapsed && runningInstanceCount > 0" class="nav-pill-badge">
            {{ runningInstanceCount }}
          </span>
        </button>

        <button
          class="nav-item"
          :class="{ active: navSelected === 'homes' }"
          :title="siderCollapsed ? t('nav.homes') : ''"
          @click="navGo('homes')"
        >
          <span class="nav-icon">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
              <polyline points="9 22 9 12 15 12 15 22" />
            </svg>
          </span>
          <span v-if="!siderCollapsed" class="nav-label">{{ t('nav.homes') }}</span>
        </button>

        <button
          class="nav-item"
          :class="{ active: navSelected === 'profiles' }"
          :title="siderCollapsed ? t('nav.profiles') : ''"
          @click="navGo('profiles')"
        >
          <span class="nav-icon">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
              <circle cx="12" cy="7" r="4" />
            </svg>
          </span>
          <span v-if="!siderCollapsed" class="nav-label">{{ t('nav.profiles') }}</span>
        </button>

        <button
          class="nav-item"
          :class="{ active: navSelected === 'plugins' }"
          :title="siderCollapsed ? t('nav.plugins') : ''"
          @click="navGo('plugins')"
        >
          <span class="nav-icon">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M4 7h3a1 1 0 0 0 1-1V5a2 2 0 0 1 4 0v1a1 1 0 0 0 1 1h3a1 1 0 0 1 1 1v3a1 1 0 0 0 1 1h1a2 2 0 0 1 0 4h-1a1 1 0 0 0-1 1v3a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-1a2 2 0 0 0-4 0v1a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1h1a2 2 0 0 0 0-4H4a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1z" />
            </svg>
          </span>
          <span v-if="!siderCollapsed" class="nav-label">{{ t('nav.plugins') }}</span>
        </button>

        <button
          class="nav-item"
          :class="{ active: navSelected === 'versions' }"
          :title="siderCollapsed ? t('nav.versions') : ''"
          @click="navGo('versions')"
        >
          <span class="nav-icon">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <line x1="16.5" y1="9.4" x2="7.5" y2="4.21" />
              <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
              <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
              <line x1="12" y1="22.08" x2="12" y2="12" />
            </svg>
          </span>
          <span v-if="!siderCollapsed" class="nav-label">{{ t('nav.versions') }}</span>
        </button>

        <button
          class="nav-item"
          :class="{ active: navSelected === 'tasks' }"
          :title="siderCollapsed ? t('nav.tasks') : ''"
          @click="navGo('tasks')"
        >
          <span class="nav-icon">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10" />
              <polyline points="12 6 12 12 16 14" />
            </svg>
          </span>
          <span v-if="!siderCollapsed" class="nav-label">{{ t('nav.tasks') }}</span>
          <span v-if="!siderCollapsed && store.runningTaskCount > 0" class="nav-pill-badge active-task">
            {{ store.runningTaskCount }}
          </span>
          <span v-if="siderCollapsed && store.runningTaskCount > 0" class="nav-dot-active" />
        </button>

        <button
          class="nav-item"
          :class="{ active: navSelected === 'settings' }"
          :title="siderCollapsed ? t('nav.settings') : ''"
          @click="navGo('settings')"
        >
          <span class="nav-icon">
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
          </span>
          <span v-if="!siderCollapsed" class="nav-label">{{ t('nav.settings') }}</span>
        </button>
      </nav>

      <!-- Sidebar Footer (Status Hint) -->
      <div v-if="!siderCollapsed" class="sider-footer">
        <div class="sider-status-row">
          <span :class="['apple-status-dot', store.runningTaskCount > 0 ? 'starting' : 'idle']">
            {{ store.runningTaskCount > 0 ? t('tasks.runningHint', { count: store.runningTaskCount }) : t('tasks.idleHint') }}
          </span>
        </div>
      </div>
    </aside>

    <!-- Main Window Area -->
    <main class="apple-main">
      <!-- Unified Header Toolbar -->
      <header class="apple-header" @mousedown="onHeaderMouseDown" @dblclick="onHeaderDoubleClick">
        <div class="header-left">
          <button
            class="header-mac-btn"
            :title="t('nav.toggleSider')"
            data-no-drag
            @click="toggleSider"
          >
            <!-- SF-Symbol-like sidebar toggle: a window frame with the sidebar
                 rail filled when it is open, and hollow when collapsed. -->
            <svg
              viewBox="0 0 16 16"
              width="15"
              height="15"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2.5" />
              <line x1="6.25" y1="2.75" x2="6.25" y2="13.25" />
              <rect
                v-if="!siderCollapsed"
                x="2.5"
                y="3.5"
                width="3"
                height="9"
                rx="1.4"
                fill="currentColor"
                stroke="none"
                opacity="0.28"
              />
            </svg>
          </button>
          <button v-if="showBack" class="header-mac-btn" :title="t('common.back')" data-no-drag @click="onHeaderBack">
            <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M10 13L5 8l5-5" />
            </svg>
          </button>
          <span class="header-title">{{ pageTitle }}</span>
          <span v-if="!isTauri" class="header-mock-chip">{{ t('app.mockBadge') }}</span>
        </div>

        <div class="header-spacer" />

        <div class="header-actions">
          <button
            class="header-mac-btn"
            :title="t('common.refresh')"
            data-no-drag
            @click="refreshShortcut"
          >
            <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
              <path d="M13.5 8A5.5 5.5 0 1 1 12 4.1L14 2" />
              <polyline points="14 5.5 14 2 10.5 2" />
            </svg>
          </button>
        </div>
      </header>

      <!-- Content Scroll Container -->
      <div class="apple-content">
        <a-scrollbar
          type="track"
          outer-style="height: 100%"
          style="height: 100%; overflow-y: auto"
        >
          <router-view />
        </a-scrollbar>
      </div>
    </main>
  </div>
</template>

<style lang="scss" scoped>
.apple-window {
  height: 100%;
  width: 100%;
  display: flex;
  flex-direction: row;
  overflow: hidden;
  background-color: var(--apple-content-bg);
}

// Unified Sidebar
.apple-sider {
  width: var(--dl-sider-width);
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  background: var(--apple-sidebar-bg);
  backdrop-filter: blur(28px) saturate(180%);
  -webkit-backdrop-filter: blur(28px) saturate(180%);
  border-right: 1px solid var(--apple-sidebar-border);
  transition: width var(--apple-duration) var(--apple-spring-curve);
  z-index: 10;

  &.collapsed {
    width: var(--dl-sider-collapsed-width);

    .nav-item {
      justify-content: center;
      padding: 9px 0;
    }
  }
}

// Window Traffic Area in Sidebar.
// A bare drag strip: the native traffic lights draw over its left end, and
// the sidebar toggle lives in the main toolbar. onHeaderMouseDown makes the
// rest of the strip draggable.
.sider-traffic-header {
  height: var(--dl-header-height);
  flex-shrink: 0;
  border-bottom: 1px solid var(--apple-separator);
  -webkit-app-region: drag;
  user-select: none;
}

// Sidebar Navigation
.sider-nav {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 11px;
  width: 100%;
  padding: 8px 12px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--color-text-2);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  text-align: left;
  position: relative;
  transition:
    background-color 0.15s ease,
    color 0.15s ease,
    transform 0.12s ease-out;

  &:hover {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &.active {
    background: rgb(var(--primary-6) / 14%);
    color: rgb(var(--primary-6));
    font-weight: 600;

    .nav-icon svg {
      stroke: rgb(var(--primary-6));
    }
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.nav-icon {
  width: 20px;
  height: 20px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;

  svg {
    stroke: currentColor;
    transition: stroke 0.15s ease;
  }
}

.nav-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nav-pill-badge {
  padding: 1px 7px;
  font-size: 11px;
  font-weight: 600;
  border-radius: 10px;
  background: rgb(var(--green-6) / 16%);
  color: rgb(var(--green-6));

  &.active-task {
    background: rgb(var(--primary-6) / 18%);
    color: rgb(var(--primary-6));
  }
}

.nav-dot-active {
  position: absolute;
  top: 7px;
  right: 10px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: rgb(var(--primary-6));
  box-shadow: 0 0 6px rgb(var(--primary-6) / 60%);
}

// Sidebar Footer
.sider-footer {
  padding: 12px 14px;
  border-top: 1px solid var(--apple-sidebar-border);
  font-size: 12px;
}

.sider-status-row {
  display: flex;
  align-items: center;
}

// Main Window Area
.apple-main {
  flex: 1;
  min-width: 0;
  height: 100%;
  display: flex;
  flex-direction: column;
  position: relative;
}

// Unified Header
.apple-header {
  height: var(--dl-header-height);
  flex-shrink: 0;
  display: flex;
  align-items: center;
  padding: 0 24px;
  border-bottom: 1px solid var(--apple-separator);
  background: var(--apple-content-bg);
  -webkit-app-region: drag;
  user-select: none;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 12px;
}

.header-title {
  font-size: 16px;
  font-weight: 600;
  letter-spacing: -0.015em;
  color: var(--color-text-1);
}

.header-mock-chip {
  padding: 2px 7px;
  font-size: 11px;
  font-weight: 600;
  border-radius: 6px;
  background: rgb(var(--orange-6) / 14%);
  color: rgb(var(--orange-6));
}

.header-spacer {
  flex: 1;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.header-mac-btn {
  border: none;
  background: transparent;
  color: var(--color-text-2);
  border-radius: 7px;
  width: 28px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.15s ease;

  &:hover {
    background: var(--apple-group-bg);
    color: var(--color-text-1);
  }

  &:active {
    transform: scale(var(--apple-active-scale));
  }
}

.apple-content {
  flex: 1;
  min-height: 0;
  overflow: hidden;
  background-color: var(--apple-content-bg);
}
</style>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useLauncherStore } from '@/stores/launcher'
import type { ThemeMode } from '@/api/types'
import { api } from '@/api'
import {
  MENU_NAVIGATE_EVENTS,
  handleAppKeydown,
  resolveMenuRoute,
  type ShortcutRoute,
} from '@/shortcuts'
import DlIcon from '@/components/DlIcon.vue'

const route = useRoute()
const router = useRouter()
const { t, locale } = useI18n()
const store = useLauncherStore()
const isTauri = api.isTauri

// --- Theme: light / dark / follow system -------------------------------------

const themeMedia = window.matchMedia('(prefers-color-scheme: dark)')

/**
 * Applies the effective Arco theme to <body> (body[arco-theme='dark']) and
 * keeps the native window in step. The title bar is drawn by macOS, not by the
 * page: without the native half, a dark app under a light OS keeps a white
 * title bar and the chrome contradicts the content.
 */
function applyTheme(mode: ThemeMode) {
  const dark = mode === 'dark' || (mode === 'system' && themeMedia.matches)
  if (dark) document.body.setAttribute('arco-theme', 'dark')
  else document.body.removeAttribute('arco-theme')
  void applyNativeTheme(mode)
}

async function applyNativeTheme(mode: ThemeMode) {
  if (!appWindow) return
  try {
    const win = await appWindow
    await win.setTheme(mode === 'system' ? null : mode)
  } catch {
    // Older runtimes or a non-Tauri context: the page theme still applies.
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

  // Land on the environment check when the toolchain is incomplete: pnpm is a
  // hard requirement for installing a version and for plugin management, and
  // the launcher no longer installs one itself. Navigation stays open, so a
  // network-restricted user can still reach Settings and configure a proxy.
  const runtimeIncomplete = !store.runtime?.node?.installed || !store.runtime?.pnpm?.installed
  if (runtimeIncomplete && route.name !== 'setup') {
    void router.push({ name: 'setup' })
  }

  await setupLaunchDeepLink()
  window.addEventListener('keydown', onAppKeydown)
  await setupMenuListeners()
})

// --- Shortcuts ----------------------------------------------------------------

function pushLibrary(tab: 'versions' | 'homes' | 'profiles' | undefined) {
  const query = tab ? { tab } : {}
  if (route.name === 'library') {
    void router.replace({ name: 'library', query })
    return
  }
  void router.push({ name: 'library', query })
}

function goShortcut(name: ShortcutRoute) {
  switch (name) {
    case 'instances':
      if (route.name === 'instance-detail') return
      void router.push({ name: 'instances' }).catch(() => undefined)
      return
    case 'library':
      pushLibrary(undefined)
      return
    case 'library:versions':
      pushLibrary('versions')
      return
    case 'library:homes':
      pushLibrary('homes')
      return
    case 'library:profiles':
      pushLibrary('profiles')
      return
    case 'settings':
      void router.push({ name: 'settings' }).catch(() => undefined)
      return
  }
}

async function refreshAll() {
  await Promise.allSettled([store.refreshInstances(), store.refreshVersions(), store.checkRuntime()])
}

function onAppKeydown(e: KeyboardEvent) {
  if (handleAppKeydown(e, { go: goShortcut, refresh: () => void refreshAll(), back: backShortcut })) {
    e.preventDefault()
  }
}

function backShortcut() {
  if (route.name === 'instances') return
  router.back()
}

// --- Backend menu events ------------------------------------------------------

let unlistenMenu: (() => void)[] = []

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
    if (info.up_to_date) Message.success(t('settings.update.upToDate'))
    else {
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
    await api.openExternal('https://github.com/tttnny/dsh-launcher')
  } catch (e) {
    Message.error(String(e))
  }
}

// --- Launch deep link: dsh-launcher://launch → start instance -----------------

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
    if (u.host === 'launch') void launchFromDeepLink(u)
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

// --- Sidebar ------------------------------------------------------------------

type NavKey = 'instances' | 'library' | 'settings'

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
  if (name === 'library') return 'library'
  if (name === 'settings' || name === 'setup') return 'settings'
  return 'instances'
})

const navGo = goShortcut

const runningCount = computed(
  () => store.instances.filter((i) => store.statusOf(i.id).state === 'running').length,
)

const envOk = computed(
  () => !!store.runtime?.node?.installed && !!store.runtime?.pnpm?.installed,
)

// --- Header -------------------------------------------------------------------

const showBack = computed(() => route.name === 'instance-detail' || route.name === 'setup')

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
// startDragging() enters a native drag loop that swallows the pending click, so
// only start dragging once the pointer moves past a small threshold — fast
// clicks then complete normally and dblclick (zoom) still reaches the webview.

let dragArmed: { x: number; y: number; started: boolean } | null = null

function onHeaderMouseDown(e: MouseEvent) {
  if (!isTauri || e.button !== 0) return
  const el = e.target as HTMLElement | null
  if (el?.closest('.dl-nav-item, a, button, input, [data-no-drag]')) return
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
  if (el?.closest('.dl-nav-item, a, button, input, [data-no-drag]')) return
  try {
    const w = await appWindow
    if (!w) return
    if (await w.isMaximized()) await w.unmaximize()
    else await w.maximize()
  } catch {
    // Advisory only
  }
}
</script>

<template>
  <div class="dl-app">
    <aside class="dl-sidebar" :class="{ 'is-collapsed': siderCollapsed }">
      <!-- Drag strip: the native traffic lights draw over its left end. -->
      <div
        class="dl-sidebar-drag"
        @mousedown="onHeaderMouseDown"
        @dblclick="onHeaderDoubleClick"
      />

      <nav class="dl-nav">
        <button
          class="dl-nav-item"
          :class="{ 'is-active': navSelected === 'instances' }"
          :title="siderCollapsed ? t('nav.instances') : ''"
          @click="navGo('instances')"
        >
          <DlIcon name="instances" :size="17" />
          <span v-if="!siderCollapsed" class="dl-nav-label">{{ t('nav.instances') }}</span>
          <span v-if="!siderCollapsed && runningCount > 0" class="dl-nav-badge">{{ runningCount }}</span>
        </button>

        <button
          class="dl-nav-item"
          :class="{ 'is-active': navSelected === 'library' }"
          :title="siderCollapsed ? t('nav.library') : ''"
          @click="navGo('library')"
        >
          <DlIcon name="library" :size="17" />
          <span v-if="!siderCollapsed" class="dl-nav-label">{{ t('nav.library') }}</span>
        </button>

        <button
          class="dl-nav-item"
          :class="{ 'is-active': navSelected === 'settings' }"
          :title="siderCollapsed ? t('nav.settings') : ''"
          @click="navGo('settings')"
        >
          <DlIcon name="settings" :size="17" />
          <span v-if="!siderCollapsed" class="dl-nav-label">{{ t('nav.settings') }}</span>
        </button>
      </nav>

      <div class="dl-sidebar-foot">
        <button
          v-if="store.healthTotalCount > 0"
          class="dl-foot-row"
          :class="store.healthErrorCount > 0 ? 'is-danger' : 'is-warn'"
          :title="t('home.healthLogs')"
          @click="navGo('instances')"
        >
          <DlIcon name="health" :size="15" />
          <span v-if="!siderCollapsed" class="dl-foot-label">{{ t('home.healthLogs') }}</span>
          <span v-if="!siderCollapsed" class="dl-foot-count">{{ store.healthTotalCount }}</span>
        </button>

        <button
          class="dl-foot-row"
          :class="{ 'is-danger': !envOk }"
          :title="t('setup.title')"
          @click="router.push({ name: 'setup' })"
        >
          <DlIcon name="terminal" :size="15" />
          <span v-if="!siderCollapsed" class="dl-foot-label">
            {{ envOk ? t('nav.envReady') : t('nav.envMissing') }}
          </span>
        </button>
      </div>
    </aside>

    <main class="dl-main">
      <header class="dl-toolbar" @mousedown="onHeaderMouseDown" @dblclick="onHeaderDoubleClick">
        <button class="dl-icon-btn" :title="t('nav.toggleSider')" data-no-drag @click="toggleSider">
          <DlIcon name="sidebar" :size="15" />
        </button>
        <button v-if="showBack" class="dl-icon-btn" :title="t('common.back')" data-no-drag @click="onHeaderBack">
          <DlIcon name="back" :size="15" />
        </button>

        <span v-if="!isTauri" class="dl-tag dl-tag--muted" style="margin-left: 4px">
          {{ t('app.mockBadge') }}
        </span>

        <span class="dl-toolbar-spacer" />
      </header>

      <div class="dl-content">
        <!-- Keyed by route name only: switching a Library tab is a query change
             on the same page, and the panes react to it without a remount. -->
        <router-view :key="String(route.name)" />
      </div>
    </main>
  </div>
</template>

<style lang="scss" scoped>
.dl-app {
  display: flex;
  height: 100%;
  width: 100%;
  overflow: hidden;
  background: var(--dl-canvas);
}

// --- Sidebar -----------------------------------------------------------------

.dl-sidebar {
  display: flex;
  flex-direction: column;
  width: var(--dl-sidebar);
  flex-shrink: 0;
  background: var(--dl-surface);
  border-right: 1px solid var(--dl-line);
  transition: width var(--dl-slow) var(--dl-ease);

  &.is-collapsed {
    width: var(--dl-sidebar-collapsed);
  }
}

.dl-sidebar-drag {
  height: var(--dl-header);
  flex-shrink: 0;
  user-select: none;
}

.dl-nav {
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 6px 10px;
}

.dl-nav-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 7px 10px;
  border: none;
  border-radius: var(--dl-r-ctl);
  background: transparent;
  color: var(--dl-ink-2);
  font-family: inherit;
  font-size: var(--dl-text-md);
  font-weight: 550;
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--dl-fast) var(--dl-ease),
    color var(--dl-fast) var(--dl-ease);

  &:hover:not(.is-active) {
    background: var(--dl-surface-2);
    color: var(--dl-ink);
  }

  &.is-active {
    background: var(--dl-signal-soft);
    color: var(--dl-signal-ink);
  }

  // The active rail is the app's one structural marker in the sidebar.
  &.is-active::before {
    content: '';
    position: absolute;
    left: -10px;
    top: 50%;
    width: 3px;
    height: 16px;
    border-radius: 0 2px 2px 0;
    background: var(--dl-signal);
    transform: translateY(-50%);
  }
}

.dl-nav-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dl-nav-badge {
  padding: 0 6px;
  border-radius: 999px;
  background: var(--dl-run-soft);
  color: var(--dl-run);
  font-size: var(--dl-text-xs);
  font-weight: 650;
  font-variant-numeric: tabular-nums;
}

.dl-sidebar-foot {
  margin-top: auto;
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 10px;
  border-top: 1px solid var(--dl-line);
}

.dl-foot-row {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  border-radius: var(--dl-r-ctl);
  background: transparent;
  color: var(--dl-ink-3);
  font-family: inherit;
  font-size: var(--dl-text-sm);
  font-weight: 500;
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--dl-fast) var(--dl-ease),
    color var(--dl-fast) var(--dl-ease);

  &:hover {
    background: var(--dl-surface-2);
    color: var(--dl-ink);
  }

  &.is-warn {
    color: var(--dl-warn);
  }

  &.is-danger {
    color: var(--dl-danger);
  }
}

.dl-foot-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dl-foot-count {
  padding: 0 6px;
  border-radius: 6px;
  background: currentColor;
  color: var(--dl-surface);
  font-size: var(--dl-text-xs);
  font-weight: 650;
}

// --- Main --------------------------------------------------------------------

.dl-main {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  height: 100%;
}

.dl-toolbar {
  display: flex;
  align-items: center;
  gap: 4px;
  height: var(--dl-header);
  flex-shrink: 0;
  padding: 0 16px;
  border-bottom: 1px solid var(--dl-line);
  background: var(--dl-surface);
  user-select: none;
}

.dl-toolbar-spacer {
  flex: 1;
}

.dl-content {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}
</style>

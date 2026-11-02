<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Message } from '@arco-design/web-vue'
import { useLauncherStore } from '@/stores/launcher'
import { api } from '@/api'
import { LOCAL_HOST_ID, type DshInstance, type ThemeMode } from '@/api/types'
import {
  MENU_NAVIGATE_EVENTS,
  handleAppKeydown,
  resolveMenuRoute,
  type ShortcutRoute,
} from '@/shortcuts'
import AppRail from '@/components/AppRail.vue'
import launcherIcon from '@/assets/launcher-icon.png'

const route = useRoute()
const router = useRouter()
const store = useLauncherStore()
const isTauri = api.isTauri
const contentEl = ref<HTMLElement | null>(null)

watch(
  () => route.fullPath,
  async () => {
    await nextTick()
    contentEl.value?.scrollTo({ top: 0 })
  },
)

// --- 主题 -------------------------------------------------------------------

const themeMedia = window.matchMedia('(prefers-color-scheme: dark)')

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
    // 浏览器预览或旧运行时：页面主题仍然生效。
  }
}

function onSystemThemeChange() {
  if (store.settings.theme === 'system') applyTheme('system')
}

onMounted(async () => {
  await store.init()
  if (store.initErrors.length) {
    Message.error(`启动时有数据读取失败：${store.initErrors.join('；')}`)
  }
  applyTheme(store.settings.theme || 'system')
  themeMedia.addEventListener('change', onSystemThemeChange)

  await setupLaunchDeepLink()
  window.addEventListener('keydown', onAppKeydown)
  await setupMenuListeners()
})

// --- 快捷键 -----------------------------------------------------------------

function goShortcut(name: ShortcutRoute) {
  const target: Record<ShortcutRoute, string> = {
    instances: '/',
    install: '/install',
    data: '/data',
    machines: '/machines',
    settings: '/settings',
  }
  void router.push(target[name]).catch(() => undefined)
}

async function refreshAll() {
  await Promise.allSettled([
    store.refreshInstances(),
    store.loadHostResources(store.currentHostId),
    store.checkRuntime(),
  ])
}

function onAppKeydown(e: KeyboardEvent) {
  if (
    handleAppKeydown(e, {
      go: goShortcut,
      refresh: () => void refreshAll(),
      back: backShortcut,
    })
  ) {
    e.preventDefault()
  }
}

function backShortcut() {
  if (route.name === 'instances') return
  router.back()
}

// --- 原生菜单 -----------------------------------------------------------------

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
    unlistenMenu.push(
      await listen('check-update', () => void onMenuCheckUpdate()),
    )
    unlistenMenu.push(await listen('open-help', () => void onMenuOpenHelp()))
  } catch {
    // 事件桥不可用时（浏览器预览）快捷键仍然可用。
  }
}

let checkingMenuUpdate = false

async function onMenuCheckUpdate() {
  if (checkingMenuUpdate) return
  checkingMenuUpdate = true
  try {
    const info = await api.checkLauncherUpdate('dev')
    if (info.up_to_date) Message.success('当前已是最新版本')
    else {
      Message.info(`发现新版本 v${info.latest ?? ''}`)
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

// --- 启动深链接 ---------------------------------------------------------------

let unlistenDeepLink: (() => void) | undefined

async function setupLaunchDeepLink() {
  if (!isTauri) return
  const pending = await api.pendingDeepLink()
  if (pending) handleDeepLink(pending)
  const { listen } = await import('@tauri-apps/api/event')
  unlistenDeepLink = await listen<string>('deep-link', (event) =>
    handleDeepLink(event.payload),
  )
}

function handleDeepLink(raw: string) {
  try {
    const u = new URL(raw)
    if (u.protocol !== 'dsh-launcher:') return
    if (u.host === 'launch') void launchFromDeepLink(u)
  } catch {
    // 忽略无效链接
  }
}

function launchTarget(ref: string): DshInstance | undefined {
  const byId = store.instanceById(ref)
  if (byId) return byId
  return store.instancesOfHost(store.currentHostId).find((i) => i.name === ref)
}

async function launchFromDeepLink(u: URL) {
  const ref = u.searchParams.get('instance')
  if (!ref) return
  const inst = launchTarget(ref)
  if (!inst) {
    Message.error(`这台机器上没有名为「${ref}」的实例`)
    return
  }
  try {
    const state = store.statusOf(inst.id).state
    if (state !== 'running' && state !== 'starting') {
      const profile =
        u.searchParams.get('profile') || inst.default_profile || 'web'
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
  store.dispose()
})

watch(
  () => store.settings.theme,
  (v) => {
    if (v) applyTheme(v)
  },
)

// --- 窗口拖动 -----------------------------------------------------------------

const appWindow = (() => {
  return (window as unknown as { __TAURI_INTERNALS__?: unknown })
    .__TAURI_INTERNALS__
    ? loadWindowApi()
    : null
})()

async function loadWindowApi() {
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  return getCurrentWindow()
}

let dragArmed: { x: number; y: number; started: boolean } | null = null

function onTitlebarMouseDown(e: MouseEvent) {
  if (!isTauri || e.button !== 0) return
  const el = e.target as HTMLElement | null
  if (el?.closest('button, a, input, [data-no-drag]')) return
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

async function onTitlebarDoubleClick(e: MouseEvent) {
  if (!isTauri || e.button !== 0) return
  const el = e.target as HTMLElement | null
  if (el?.closest('button, a, input, [data-no-drag]')) return
  try {
    const w = await appWindow
    if (!w) return
    if (await w.isMaximized()) await w.unmaximize()
    else await w.maximize()
  } catch {
    // 仅影响窗口缩放，忽略。
  }
}
</script>

<template>
  <div v-if="!store.loaded" class="app-boot">
    <img class="app-boot-mark" :src="launcherIcon" alt="" />
    <span class="app-boot-title">DSH Launcher</span>
    <span class="app-boot-hint">正在读取启动器数据…</span>
  </div>

  <div v-else class="app-shell">
    <AppRail />
    <main class="app-main">
      <div
        class="app-titlebar"
        @mousedown="onTitlebarMouseDown"
        @dblclick="onTitlebarDoubleClick"
      ></div>
      <div ref="contentEl" class="app-scroll">
        <router-view :key="String(route.name)" />
      </div>
    </main>
  </div>
</template>

<style lang="scss" scoped>
.app-boot {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  width: 100%;
  height: 100%;
  background: var(--dl-bg);
  color: var(--dl-ink);
}

.app-boot-mark {
  width: 48px;
  height: 48px;
  margin-bottom: 8px;
  border-radius: 12px;
}

.app-boot-title {
  font-family: var(--dl-font-display);
  font-size: 20px;
  font-weight: 600;
}

.app-boot-hint {
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.app-shell {
  display: flex;
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: var(--dl-bg);
}

.app-main {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  height: 100%;
}

.app-titlebar {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  height: 26px;
  flex-shrink: 0;
  padding: 0 16px;
  user-select: none;
}

.app-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}
</style>

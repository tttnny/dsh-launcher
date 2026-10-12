/**
 * In-app keyboard shortcuts + backend menu-event contract (t4).
 *
 * The Rust side (t3, Mac native menu / global shortcuts) emits Tauri events
 * to drive the frontend; this module is the single place that defines the
 * event names and the route mapping so both sides stay in sync:
 *
 * - navigate events: `{ route: string }` or a plain route-name string.
 *   Accepted routes: home / instances / homes / profiles / versions / settings.
 *   `/plugins` is accepted as a legacy alias and lands on profiles.
 * - refresh events: no payload; the frontend re-reads instance status.
 *
 * Multiple event names are accepted for forward/backward compatibility with
 * whichever name t3 finally emits (`menu-navigate` is canonical).
 */

export type ShortcutRoute = 'home' | 'instances' | 'homes' | 'profiles' | 'versions' | 'settings'

/** Canonical first, aliases after. */
export const MENU_NAVIGATE_EVENTS = ['menu-navigate', 'navigate', 'app-navigate'] as const

export const MENU_REFRESH_EVENTS = ['menu-refresh', 'app-refresh'] as const

/**
 * Single source of truth for the Settings shortcut card (t14).
 * `labelKey` is a vue-i18n key under `settings.shortcuts`; `keys` are the
 * display chips. Rows marked `native` are handled by the OS/Rust menu
 * (app_menu.rs) rather than `handleAppKeydown`, but are listed so the card
 * documents the full table and cannot drift from it.
 */
export interface ShortcutDoc {
  labelKey: string
  keys: string[]
  native?: boolean
}

const MOD = '\u2318 / Ctrl'

export const SHORTCUT_DOCS: ShortcutDoc[] = [
  { labelKey: 'settings.shortcuts.goHome', keys: [MOD, '1'] },
  { labelKey: 'settings.shortcuts.goHomes', keys: [MOD, '2'] },
  { labelKey: 'settings.shortcuts.goProfiles', keys: [MOD, '3'] },
  { labelKey: 'settings.shortcuts.goVersions', keys: [MOD, '4'] },
  { labelKey: 'settings.shortcuts.openSettings', keys: [MOD, ','] },
  { labelKey: 'settings.shortcuts.showMain', keys: [MOD, '0'], native: true },
  { labelKey: 'settings.shortcuts.quitApp', keys: [MOD, 'Q'], native: true },
  { labelKey: 'settings.shortcuts.refresh', keys: [MOD, 'R'] },
  { labelKey: 'settings.shortcuts.back', keys: ['Esc'] },
]

const NAVIGATE_TARGETS: Record<string, ShortcutRoute> = {
  home: 'home',
  '/': 'home',
  settings: 'settings',
  '/settings': 'settings',
  instances: 'home',
  '/instances': 'home',
  homes: 'homes',
  '/homes': 'homes',
  profiles: 'profiles',
  '/profiles': 'profiles',
  versions: 'versions',
  '/versions': 'versions',
}

/** Normalises a menu-event payload to a known route, or null when unknown. */
export function resolveMenuRoute(payload: unknown): ShortcutRoute | null {
  let raw: unknown = payload
  if (
    payload !== null &&
    typeof payload === 'object' &&
    ('route' in (payload as Record<string, unknown>) || 'name' in (payload as Record<string, unknown>))
  ) {
    const obj = payload as Record<string, unknown>
    raw = obj.route ?? obj.name
  }
  if (typeof raw !== 'string') return null
  const key = raw.trim()
  const direct = NAVIGATE_TARGETS[key]
  if (direct) return direct
  // instance-edit maps to the instance list.
  // t6: strip leading slashes first — backend emits full paths such as
  // `/instances/:id`, which otherwise miss the match.
  const dir = key.replace(/^\/+/, '').replace(/\/+$/, '')
  // `/plugins` is no longer a top-level page: the plugin manager is reached
  // from a Profile's own row (and from the tray), so both the old in-app route
  // and the tray/menu payload land on the Profile list that owns the button.
  if (dir === 'plugins' || dir.startsWith('plugins/')) return 'profiles'
  if (dir.startsWith('instance')) return 'instances'
  return null
}

export interface ShortcutActions {
  go: (name: ShortcutRoute) => void
  refresh: () => void
  back: () => void
}

/** True when the keydown originated inside a text-editing control. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  const tag = target.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT'
}

/**
 * Global keydown dispatcher for in-app shortcuts.
 *
 * Digit mapping mirrors the native menu (app_menu.rs "显示" submenu):
 * - Cmd/Ctrl+1 → home, Cmd/Ctrl+2 → homes, Cmd/Ctrl+3 → profiles,
 *   Cmd/Ctrl+4 → versions
 * - Cmd/Ctrl+, → settings (macOS Preferences convention)
 * - Cmd/Ctrl+R → in-app status refresh (NOT a page reload)
 * - Esc → blur an input, otherwise navigate back
 *
 * Editing keys inside inputs are never hijacked: every combo above requires
 * Cmd/Ctrl except Esc, and Esc inside a field only blurs instead of
 * navigating. Returns true when the event was handled.
 */
export function handleAppKeydown(e: KeyboardEvent, actions: ShortcutActions): boolean {
  if (e.defaultPrevented) return false
  // Tauri webviews may deliver key events for IME composition; ignore them.
  if (e.isComposing || e.keyCode === 229) return false

  const target = e.target as EventTarget | null
  const inField = isEditableTarget(target)

  if (e.key === 'Escape') {
    if (inField) {
      if (target instanceof HTMLElement) target.blur()
      return true
    }
    actions.back()
    return true
  }

  const mod = e.metaKey || e.ctrlKey
  if (!mod || e.altKey) return false
  // Shift+Cmd+digit produces symbols on some layouts; only bare combos count.
  if (e.shiftKey) return false

    switch (e.key) {
      case '1':
        actions.go('home')
        return true
      case '2':
        actions.go('homes')
        return true
      case '3':
        actions.go('profiles')
        return true
      case '4':
        actions.go('versions')
        return true
      case ',':
        actions.go('settings')
        return true
      case 'r':
      case 'R':
        actions.refresh()
        return true
      default:
        return false
    }
}

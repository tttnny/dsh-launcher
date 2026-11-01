/**
 * In-app keyboard shortcuts + the backend menu-event contract.
 *
 * The Rust side (native menu bar, tray) emits Tauri events carrying a route
 * payload; this module is the single place that defines the event names and
 * the payload → route mapping, so the two sides cannot drift.
 *
 * The app has four top-level pages now, so the digits address pages and
 * Library tabs:
 *   ⌘1 instances · ⌘2 library/versions · ⌘3 homes · ⌘4 profiles · ⌘5 hosts ·
 *   ⌘, settings
 * Legacy payloads (`/homes`, `/profiles`, `/versions`, `/plugins`) keep
 * working — they land on the matching Library tab.
 */

export type ShortcutRoute =
  | 'instances'
  | 'library'
  | 'hosts'
  | 'library:versions'
  | 'library:homes'
  | 'library:profiles'
  | 'settings'

/** Canonical first, aliases after. */
export const MENU_NAVIGATE_EVENTS = [
  'menu-navigate',
  'navigate',
  'app-navigate',
] as const

/**
 * Single source of truth for the Settings shortcut card. `label` is the row's
 * text; `keys` are the display chips. Rows marked `native` are handled by the
 * OS/Rust menu rather than `handleAppKeydown`, but are listed so the card
 * cannot drift from the table.
 */
export interface ShortcutDoc {
  label: string
  keys: string[]
  native?: boolean
}

/** The one modifier this app ships with: the macOS Command key. */
const MOD = '\u2318'

export const SHORTCUT_DOCS: ShortcutDoc[] = [
  { label: '前往实例', keys: [MOD, '1'] },
  { label: '前往 DSH 版本', keys: [MOD, '2'] },
  { label: '前往 DSH_HOME', keys: [MOD, '3'] },
  { label: '前往 Profile', keys: [MOD, '4'] },
  { label: '前往主机', keys: [MOD, '5'] },
  { label: '打开设置', keys: [MOD, ','] },
  { label: '刷新状态', keys: [MOD, 'R'] },
  { label: '返回上一页（输入框内仅退出输入）', keys: ['Esc'] },
  { label: '显示主窗口', keys: [MOD, '0'], native: true },
  { label: '退出启动器', keys: [MOD, 'Q'], native: true },
]

/** Backend route payload → the route this app should render. */
export function resolveMenuRoute(payload: unknown): ShortcutRoute | null {
  let raw: unknown = payload
  if (
    payload !== null &&
    typeof payload === 'object' &&
    ('route' in (payload as Record<string, unknown>) ||
      'name' in (payload as Record<string, unknown>))
  ) {
    const obj = payload as Record<string, unknown>
    raw = obj.route ?? obj.name
  }
  if (typeof raw !== 'string') return null

  // Strip any query the backend appended (`/library?tab=homes`).
  const [path, query] = raw.trim().split('?')
  const key = path.replace(/^\/+/, '').replace(/\/+$/, '')
  const tab = new URLSearchParams(query ?? '').get('tab')
  if (tab === 'versions' || tab === 'homes' || tab === 'profiles') {
    return `library:${tab}` as ShortcutRoute
  }

  switch (key) {
    case '':
    case 'instances':
    case 'home':
      return 'instances'
    case 'homes':
      return 'library:homes'
    case 'profiles':
    case 'plugins':
      return 'library:profiles'
    case 'versions':
    case 'tasks':
      return 'library:versions'
    case 'library':
      return 'library'
    case 'hosts':
      return 'hosts'
    case 'settings':
      return 'settings'
    default:
      // `instances/:id` and anything else instance-shaped lands on the list.
      return key.startsWith('instance') ? 'instances' : null
  }
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
 * Every combo requires the platform modifier except Esc, so plain typing is
 * never hijacked; Esc inside a field only blurs instead of navigating. Returns
 * true when the event was handled.
 */
export function handleAppKeydown(
  e: KeyboardEvent,
  actions: ShortcutActions,
): boolean {
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
  if (!mod || e.altKey || e.shiftKey) return false

  switch (e.key.toLowerCase()) {
    case '1':
      actions.go('instances')
      return true
    case '2':
      actions.go('library:versions')
      return true
    case '3':
      actions.go('library:homes')
      return true
    case '4':
      actions.go('library:profiles')
      return true
    case '5':
      actions.go('hosts')
      return true
    case ',':
      actions.go('settings')
      return true
    case 'r':
      actions.refresh()
      return true
    default:
      return false
  }
}

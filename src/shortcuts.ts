export type ShortcutRoute =
  'instances' | 'install' | 'data' | 'machines' | 'settings'

export const MENU_NAVIGATE_EVENTS = [
  'menu-navigate',
  'navigate',
  'app-navigate',
] as const

export interface ShortcutDoc {
  label: string
  keys: string[]
  native?: boolean
}

const MOD = '⌘'

export const SHORTCUT_DOCS: ShortcutDoc[] = [
  { label: '前往实例', keys: [MOD, '1'] },
  { label: '前往安装', keys: [MOD, '2'] },
  { label: '前往数据', keys: [MOD, '3'] },
  { label: '前往机器', keys: [MOD, '4'] },
  { label: '打开设置', keys: [MOD, ','] },
  { label: '刷新当前数据', keys: [MOD, 'R'] },
  { label: '返回上一页（输入框内仅退出输入）', keys: ['Esc'] },
  { label: '显示主窗口', keys: [MOD, '0'], native: true },
  { label: '退出启动器', keys: [MOD, 'Q'], native: true },
]

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

  const [path, query] = raw.trim().split('?')
  const key = path.replace(/^\/+/, '').replace(/\/+$/, '')
  const tab = new URLSearchParams(query ?? '').get('tab')
  if (key === 'library') {
    return tab === 'versions' ? 'install' : 'data'
  }

  switch (key) {
    case '':
    case 'instances':
    case 'home':
      return 'instances'
    case 'install':
    case 'versions':
    case 'tasks':
      return 'install'
    case 'data':
    case 'homes':
    case 'profiles':
      return 'data'
    case 'machines':
    case 'hosts':
      return 'machines'
    case 'settings':
      return 'settings'
    default:
      return key.startsWith('instance') ? 'instances' : null
  }
}

export interface ShortcutActions {
  go: (name: ShortcutRoute) => void
  refresh: () => void
  back: () => void
}

export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  const tag = target.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT'
}

export function handleAppKeydown(
  e: KeyboardEvent,
  actions: ShortcutActions,
): boolean {
  if (e.defaultPrevented) return false
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
      actions.go('install')
      return true
    case '3':
      actions.go('data')
      return true
    case '4':
      actions.go('machines')
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

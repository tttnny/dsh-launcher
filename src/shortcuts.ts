/**
 * 应用内快捷键与原生菜单事件契约。
 *
 * 原生菜单栏和托盘发出的导航事件统一走 `menu-navigate`，载荷是路由字符串；
 * 这个文件是唯一把事件名与「载荷 → 页面」写在一起的地方，前后端不会各说各话。
 */

export type ShortcutRoute =
  'instances' | 'install' | 'data' | 'machines' | 'settings'

/** 规范事件名在前，历史别名在后。 */
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

/** 原生菜单载荷 → 新页面。旧路径一并兼容，避免托盘和历史链接失效。 */
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

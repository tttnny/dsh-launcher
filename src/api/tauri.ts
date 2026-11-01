// Tauri adapter: the desktop-shell side of the invoke seam. `call` routes to
// the real backend via invoke, or to the mock adapter in a plain browser; the
// selection happens here, exactly once.

import type { InstanceStatus, TaskProgress, TaskLog } from './types'
import type { Listener } from './mock'

export const isTauri =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

export async function call<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (isTauri) {
    const { invoke } = await import('@tauri-apps/api/core')
    return invoke<T>(cmd, args)
  }
  // The browser preview's reference backend exists only in development: a
  // production build has no reference implementation to load, so Vite drops
  // this branch and the module with it.
  if (!import.meta.env.DEV) {
    throw new Error(`浏览器预览仅在开发模式可用：${cmd}`)
  }
  const { mockCall } = await import('./mock')
  return mockCall<T>(cmd, args)
}

// ---------------------------------------------------------------------------
// Event bridge (the Tauri half of the event seam)
// ---------------------------------------------------------------------------

export async function subscribeTauriInstanceStatus(
  cb: Listener<InstanceStatus>,
): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event')
  return listen<InstanceStatus>('instance://status', (e) => cb(e.payload))
}

export async function subscribeTauriTaskProgress(
  cb: Listener<TaskProgress>,
): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event')
  return listen<TaskProgress>('task://progress', (e) => cb(e.payload))
}

export async function subscribeTauriTaskLog(
  cb: Listener<TaskLog>,
): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event')
  return listen<TaskLog>('task://log', (e) => cb(e.payload))
}

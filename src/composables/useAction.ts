import { reactive } from 'vue'
import { Message } from '@arco-design/web-vue'

export interface UseActionOptions<T, TArgs extends unknown[]> {
  /** Groups the busy flag per invocation target (e.g. an instance id);
   *  omit for a single global bucket. */
  key?: (...args: TArgs) => string
  /** Produces the success toast text from the result; omit for silent
   *  success. */
  success?: (result: T) => string
}

export interface Action<T, TArgs extends unknown[]> {
  /** Busy per key — bind as `busy[inst.id]` in templates. The '*' bucket is
   *  the single (unkeyed) one. */
  busy: Record<string, boolean>
  /** Runs the operation; a no-op while its key is already busy. Returns the
   *  result, or undefined on failure. */
  run: (...args: TArgs) => Promise<T | undefined>
}

/**
 * Wraps one async operation with the launcher's standard invocation shape:
 * per-key busy tracking, a success toast, and a failure toast carrying the
 * backend's message. The error presentation policy (backend errors surface
 * raw, since they are already localized Chinese) lives here instead of ~45
 * view handlers.
 *
 * The busy flag doubles as a re-entrancy guard per key: `run` is a no-op
 * while that key's invocation is in flight.
 */
export function useAction<T, TArgs extends unknown[]>(
  op: (...args: TArgs) => Promise<T>,
  options: UseActionOptions<T, TArgs> = {},
): Action<T, TArgs> {
  const busy = reactive<Record<string, boolean>>({})
  const bucket = (...args: TArgs): string =>
    options.key ? options.key(...args) : '*'

  async function run(...args: TArgs): Promise<T | undefined> {
    const k = bucket(...args)
    if (busy[k]) return undefined
    busy[k] = true
    try {
      const result = await op(...args)
      if (options.success) Message.success(options.success(result))
      return result
    } catch (e) {
      Message.error(String(e))
      return undefined
    } finally {
      busy[k] = false
    }
  }

  return { busy, run }
}

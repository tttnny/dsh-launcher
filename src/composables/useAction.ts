import { reactive } from 'vue'
import { Message } from '@arco-design/web-vue'

export interface UseActionOptions<T, TArgs extends unknown[]> {
  key?: (...args: TArgs) => string
  success?: (result: T) => string
}

export interface Action<T, TArgs extends unknown[]> {
  busy: Record<string, boolean>
  run: (...args: TArgs) => Promise<T | undefined>
}

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

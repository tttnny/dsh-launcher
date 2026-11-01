import { Message } from '@arco-design/web-vue'

/**
 * One clipboard write, with the launcher's toast policy.
 *
 * This exists because the obvious one-liner is wrong here: a WKWebView write
 * needs transient activation and *rejects* without it, so a fire-and-forget
 * `navigator.clipboard.writeText(x)` followed by a success toast reports
 * 「已复制」for a clipboard that did not change. Every copy path in the app
 * goes through here so the outcome shown is the real one.
 *
 * `successMessage` lets a caller name what was copied (e.g. a URL vs a log
 * dump); failures always use the shared message.
 */
export function useCopy() {
  async function copy(
    text: string,
    successMessage = '已复制',
  ): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(text)
      Message.success(successMessage)
      return true
    } catch {
      Message.error('复制失败，请手动选择后复制')
      return false
    }
  }

  return { copy }
}

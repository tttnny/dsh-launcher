import { Message } from '@arco-design/web-vue'
import { useI18n } from 'vue-i18n'

/**
 * One clipboard write, with the launcher's toast policy.
 *
 * This exists because the obvious one-liner is wrong here: a WKWebView write
 * needs transient activation and *rejects* without it, so a fire-and-forget
 * `navigator.clipboard.writeText(x)` followed by a success toast reports
 * 「已复制」for a clipboard that did not change. Every copy path in the app
 * goes through here so the outcome shown is the real one.
 *
 * `successKey` lets a caller name what was copied (e.g. a URL vs a log dump);
 * failures always use the shared key.
 */
export function useCopy() {
  const { t } = useI18n()

  async function copy(text: string, successKey = 'common.copied'): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(text)
      Message.success(t(successKey))
      return true
    } catch {
      Message.error(t('common.copyFailed'))
      return false
    }
  }

  return { copy }
}

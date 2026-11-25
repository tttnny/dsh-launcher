import { Message } from '@arco-design/web-vue'

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

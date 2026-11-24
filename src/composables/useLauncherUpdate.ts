import { computed, ref } from 'vue'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { LauncherUpdateInfo } from '@/api/types'

const updateInfo = ref<LauncherUpdateInfo | null>(null)
const checking = ref(false)
const downloadPercent = ref<number | null>(null)
const updateLabel = computed(() => {
  if (downloadPercent.value === 100) return '100% · 安装包已打开'
  if (downloadPercent.value !== null) return `下载中 ${downloadPercent.value}%`
  return updateInfo.value?.has_installer ? '更新' : '查看版本'
})
let pendingCheck: Promise<LauncherUpdateInfo> | undefined

async function checkUpdate(notify = true) {
  if (!pendingCheck) {
    checking.value = true
    pendingCheck = api
      .checkLauncherUpdate()
      .then((info) => {
        // Keep progress tied to the selected version until its download finishes.
        if (downloadPercent.value === null || downloadPercent.value === 100) {
          if (info.latest !== updateInfo.value?.latest)
            downloadPercent.value = null
          updateInfo.value = info
        }
        return info
      })
      .finally(() => {
        checking.value = false
        pendingCheck = undefined
      })
  }
  try {
    const info = await pendingCheck
    if (!notify) return
    if (info.up_to_date) Message.success('当前已是最新版本')
    else Message.info(`发现新版本 v${info.latest}`)
  } catch (error) {
    if (notify) Message.error(String(error))
  }
}

async function startUpdate() {
  const info = updateInfo.value
  if (!info?.latest || downloadPercent.value !== null) return
  try {
    if (!info.has_installer) {
      if (info.url) await api.openExternal(info.url)
      return
    }
    downloadPercent.value = 0
    await api.downloadLauncherUpdate(info.latest, (percent) => {
      if (downloadPercent.value !== null && percent > downloadPercent.value) {
        downloadPercent.value = percent
      }
    })
    downloadPercent.value = 100
    Message.success('安装包已下载，请在打开的窗口中完成安装')
  } catch (error) {
    downloadPercent.value = null
    Message.error(String(error))
  }
}

export function useLauncherUpdate() {
  return {
    updateInfo,
    checking,
    checkUpdate,
    startUpdate,
    downloadPercent,
    updateLabel,
  }
}

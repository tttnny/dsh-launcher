import { computed, ref } from 'vue'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { LauncherUpdateInfo } from '@/api/types'

const updateInfo = ref<LauncherUpdateInfo | null>(null)
const checking = ref(false)
const downloadPercent = ref<number | null>(null)
const readyToRestart = ref(false)
const restarting = ref(false)
const updateLabel = computed(() => {
  if (restarting.value) return '正在安装并重启…'
  if (readyToRestart.value) return '重启'
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
        if (downloadPercent.value === null && !readyToRestart.value) {
          updateInfo.value = info
          readyToRestart.value = info.ready_to_restart
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
  if (!info?.latest || downloadPercent.value !== null || restarting.value)
    return
  if (readyToRestart.value) {
    restarting.value = true
    try {
      await api.restartLauncherUpdate()
      if (!api.isTauri) {
        readyToRestart.value = false
        updateInfo.value = null
      }
    } catch (error) {
      if (error && typeof error === 'object' && 'action' in error) {
        if (error.action === 'download' || error.action === 'manual') {
          readyToRestart.value = false
        }
        if (error.action === 'manual') {
          updateInfo.value = { ...info, has_installer: false }
        }
      }
      Message.error(
        error && typeof error === 'object' && 'message' in error
          ? String(error.message)
          : String(error),
      )
    } finally {
      restarting.value = false
    }
    return
  }
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
    readyToRestart.value = true
    Message.success('更新已下载，点击「重启」安装新版本')
  } catch (error) {
    Message.error(String(error))
  } finally {
    downloadPercent.value = null
  }
}

export function useLauncherUpdate() {
  return {
    updateInfo,
    checking,
    checkUpdate,
    startUpdate,
    downloadPercent,
    readyToRestart,
    restarting,
    updateLabel,
  }
}

import { defineStore } from 'pinia'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type {
  DshHome,
  DshInstance,
  DshVersion,
  InstanceStatus,
  LauncherSettings,
  RemoteVersion,
  RuntimeStatus,
  TaskInfo,
} from '@/api/types'

export interface HealthLogItem {
  id: string
  timestamp: number
  instanceId: string
  instanceName: string
  profile: string
  level: 'warn' | 'error'
  code: string
  message: string
}

interface LauncherState {
  homes: DshHome[]
  versions: DshVersion[]
  instances: DshInstance[]
  settings: LauncherSettings
  statusById: Record<string, InstanceStatus>
  /**
   * Versions an install task is currently fetching, so the Versions page can
   * show the progress where the version itself lives. The launcher installs
   * nothing else, and `instance_id` is no longer read by any view: a finished
   * install's outcome is the instance that appears in the list.
   */
  installingVersions: Record<string, TaskInfo>
  /** Install calls whose task the backend has not listed yet, by task id. */
  pendingInstallTasks: Record<string, string>
  remoteVersions: RemoteVersion[]
  remoteLoading: boolean
  runtime: RuntimeStatus | null
  healthLogs: HealthLogItem[]
  loaded: boolean
}

export const useLauncherStore = defineStore('launcher', {
  state: (): LauncherState => ({
    homes: [],
    versions: [],
    instances: [],
    settings: {
      locale: 'zh-CN',
      minimize_to_tray: true,
      autostart: false,
      last_instance_id: null,
      theme: 'system',
      log_level: 'info',
      terminal: 'system',
      proxy_enabled: false,
      proxy_url: 'http://127.0.0.1',
      proxy_port: 7890,
      no_proxy: '127.0.0.1,localhost,::1',
      proxy_apply_dsh: false,
    },
    statusById: {},
    installingVersions: {},
    pendingInstallTasks: {},
    remoteVersions: [],
    remoteLoading: false,
    runtime: null,
    healthLogs: [],
    loaded: false,
  }),

  getters: {
    versionById: (s) => (id: string) => s.versions.find((v) => v.id === id),
    homeById: (s) => (id: string) => s.homes.find((h) => h.id === id),
    instanceById: (s) => (id: string) => s.instances.find((i) => i.id === id),
    // Referential integrity: "who uses this HOME / version" is answered here,
    // once, instead of being re-derived per view.
    instancesOfHome: (s) => (homeId: string) => s.instances.filter((i) => i.home_id === homeId),
    instancesOfVersion: (s) => (versionId: string) =>
      s.instances.filter((i) => i.version_id === versionId),
    isHomeShared: (s) => (homeId: string) =>
      s.instances.some((i) => i.home_id === homeId) &&
      s.instances.filter((i) => i.home_id === homeId).length > 1,
    statusOf: (s) => (id: string): InstanceStatus =>
      s.statusById[id] ?? { id, state: 'stopped', url: null, profile: null, exit_code: null },
    healthErrorCount: (s) => s.healthLogs.filter((l) => l.level === 'error').length,
    healthWarnCount: (s) => s.healthLogs.filter((l) => l.level === 'warn').length,
    healthTotalCount: (s) => s.healthLogs.length,
  },

  actions: {
    async init() {
      // Attach the status listener BEFORE the initial snapshot fetch, and
      // buffer events until the snapshot is applied. Otherwise an exit event
      // fired between fetch and listener attach is lost, leaving the UI
      // showing "running" for a process the backend already forgot (stop then
      // reported "实例未在运行").
      const pending: InstanceStatus[] = []
      let live = false
      const applyStatus = (st: InstanceStatus) => {
        if (st.state === 'stopped' || st.state === 'exited') {
          delete this.statusById[st.id]
        } else {
          this.statusById[st.id] = st
        }
      }
      await api.onInstanceStatus((st) => {
        if (live) applyStatus(st)
        else pending.push(st)
      })

      const [homes, versions, instances, settings, statuses, runtime, tasks] = await Promise.all([
        api.listHomes(),
        api.listVersions(),
        api.listInstances(),
        api.getSettings(),
        api.listInstanceStatus(),
        api.getRuntimeStatus(),
        // The task list is the install ledger. It is fetched after the rest
        // because a task that just finished is an instance that must be in
        // `this.instances` before its progress row is retired.
        api.listTasks(),
      ])
      this.homes = homes
      this.versions = versions
      this.instances = instances
      this.settings = settings
      this.statusById = Object.fromEntries(statuses.map((st) => [st.id, st]))
      this.runtime = runtime
      this.installingVersions = Object.fromEntries(
        tasks.filter((t) => t.kind === 'install-version' && t.state === 'running').map((t) => [t.id, t]),
      )
      this.loaded = true
      live = true
      pending.forEach(applyStatus)

      // Only install progress is tracked, and only while it runs: an install
      // whose event never arrived is recovered by `watchInstallTask` when the
      // Versions page starts one, and one that finished without an event is
      // retired on the next refresh. Nothing here invents an instance — the
      // backend owns that, and the list below is re-read once it says so.
      await api.onTaskProgress((p) => {
        const existing = this.installingVersions[p.id]
        if (existing) {
          if (p.state === 'running') {
            existing.percent = p.percent
            existing.message = p.message
            return
          }
          delete this.installingVersions[p.id]
        } else if (p.state === 'running' && this.pendingInstallTasks[p.id] !== undefined) {
          const version = this.pendingInstallTasks[p.id]
          delete this.pendingInstallTasks[p.id]
          this.installingVersions[p.id] = {
            id: p.id,
            kind: 'install-version',
            label: version,
            version,
            state: 'running',
            percent: p.percent,
            created_at: Date.now(),
            message: p.message,
            instance_id: p.instance_id,
            instance_name: null,
            logs: [],
          }
          return
        } else {
          // A task this launcher is not showing — an instance being launched
          // from elsewhere, or one this process list missed. Keep watching it
          // (the backend answered with its id) so its progress still lands.
          this.pendingInstallTasks[p.id] = ''
        }
        // An install that finished may have created an instance and a version
        // directory: re-read both, then retire the progress row.
        this.refreshInstances()
        this.refreshVersions()
        this.refreshHomes()
      })

      await api.onTaskLog((l) => {
        const existing = this.installingVersions[l.id]
        if (!existing) return
        if (existing.logs.length >= 1000) existing.logs.shift()
        existing.logs.push(l.line)
      })
    },

    async refreshInstances() {
      this.instances = await api.listInstances()
    },
    async refreshVersions() {
      this.versions = await api.listVersions()
    },
    async refreshHomes() {
      this.homes = await api.listHomes()
    },
    /** Re-reads the install ledger the Versions page renders. */
    async refreshInstallTasks() {
      const tasks = await api.listTasks()
      this.installingVersions = Object.fromEntries(
        tasks.filter((t) => t.kind === 'install-version' && t.state === 'running').map((t) => [t.id, t]),
      )
    },

    /**
     * Follows one install the user just started, so its progress appears on the
     * Versions page before the first task event arrives. The task id is what
     * the start call returned.
     */
    async watchInstallTask(taskId: string, version: string) {
      const tasks = await api.listTasks()
      const task = tasks.find((t) => t.id === taskId)
      if (task) {
        this.installingVersions[taskId] = task
        return
      }
      this.pendingInstallTasks[taskId] = version
    },

    async checkRuntime() {
      this.runtime = await api.getRuntimeStatus()
    },

    async refreshRemoteVersions() {
      this.remoteLoading = true
      try {
        this.remoteVersions = await api.fetchAvailableVersions()
      } catch (e) {
        Message.error(String(e))
      } finally {
        this.remoteLoading = false
      }
    },

    addHealthLogs(logs: HealthLogItem[]) {
      this.healthLogs.unshift(...logs)
      if (this.healthLogs.length > 500) {
        this.healthLogs.splice(500)
      }
    },

    clearHealthLogs() {
      this.healthLogs = []
    },
  },
})

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
  tasks: Record<string, TaskInfo>
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
      preserve_symlinks: false,
    },
    statusById: {},
    tasks: {},
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
    taskList: (s) => Object.values(s.tasks).sort((a, b) => b.created_at - a.created_at),
    healthErrorCount: (s) => s.healthLogs.filter((l) => l.level === 'error').length,
    healthWarnCount: (s) => s.healthLogs.filter((l) => l.level === 'warn').length,
    healthTotalCount: (s) => s.healthLogs.length,
    runningTaskCount: (s) => Object.values(s.tasks).filter((t) => t.state === 'running').length,
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

      const [homes, versions, instances, settings, statuses, tasks, runtime] = await Promise.all([
        api.listHomes(),
        api.listVersions(),
        api.listInstances(),
        api.getSettings(),
        api.listInstanceStatus(),
        api.listTasks(),
        api.getRuntimeStatus(),
      ])
      this.homes = homes
      this.versions = versions
      this.instances = instances
      this.settings = settings
      this.statusById = Object.fromEntries(statuses.map((st) => [st.id, st]))
      this.tasks = Object.fromEntries(tasks.map((t) => [t.id, t]))
      this.runtime = runtime
      this.loaded = true
      live = true
      pending.forEach(applyStatus)

      await api.onTaskProgress((p) => {
        const existing = this.tasks[p.id]
        if (existing) {
          existing.state = p.state
          existing.percent = p.percent
          existing.message = p.message
          existing.instance_id = p.instance_id
        } else {
          // Event arrived before the task list did: seed a minimal entry so
          // the task manager never misses a just-created task.
          this.tasks[p.id] = {
            id: p.id,
            kind: 'create-instance',
            label: '',
            version: '',
            state: p.state,
            percent: p.percent,
            created_at: Date.now(),
            message: p.message,
            instance_id: p.instance_id,
            instance_name: null,
            logs: [],
          }
        }
        // A create-instance task finished: refresh instance/version lists.
        if (p.state === 'done' && p.instance_id) {
          this.refreshInstances()
          this.refreshVersions()
          this.refreshHomes()
        }
      })

      await api.onTaskLog((l) => {
        const existing = this.tasks[l.id]
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
    async refreshTasks() {
      const tasks = await api.listTasks()
      this.tasks = Object.fromEntries(tasks.map((t) => [t.id, t]))
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

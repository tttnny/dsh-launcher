import { defineStore } from 'pinia'
import { api } from '@/api'
import { LOCAL_HOST_ID } from '@/api/types'
import type {
  DshHome,
  DshInstance,
  DshVersion,
  HostWithStatus,
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

/**
 * The install ledger: versions an install task is currently fetching, keyed by
 * version (a version has at most one install running). Accepts the full task
 * list, so both the initial fetch and a re-read build it the same way.
 */
function installLedger(tasks: TaskInfo[]): Record<string, TaskInfo> {
  return Object.fromEntries(
    tasks.filter((t) => t.kind === 'install-version' && t.state === 'running').map((t) => [t.version, t]),
  )
}

interface LauncherState {
  /** Every machine the launcher manages, with the status last seen for it. */
  hosts: HostWithStatus[]
  /**
   * 当前主机 — the **one** current host (ADR-0006 §3): the Library's three tabs
   * always show it, and the instances page falls back to it when its 全部 view
   * is off. Never `'all'`: the Library has no such shelf, and a second place
   * keeping its own idea of "which machine am I looking at" is what the ADR
   * forbids.
   */
  currentHostId: string
  /** The instances page's 全部 view switch (on by default): every host at once. */
  allHosts: boolean
  /** DSH_HOMEs per host. Records, so reading them never touches the machine. */
  homesByHost: Record<string, DshHome[]>
  /** Installed versions per host, as that host's own `/versions` answered. */
  versionsByHost: Record<string, DshVersion[]>
  /**
   * Why one host's resources could not be read (「连不上」 and friends), by host
   * id. A host that failed has **no** list here: showing the previous one would
   * present yesterday's answer as today's.
   */
  hostErrors: Record<string, string>
  /**
   * Instances of every host. The board labels each row with its machine, so two
   * instances called the same thing on two hosts stay two rows; nothing else
   * reads this list except through the getters below.
   */
  instances: DshInstance[]
  settings: LauncherSettings
  statusById: Record<string, InstanceStatus>
  /**
   * Versions an install task is currently fetching, keyed by version, so the
   * Versions page can show the progress where the version itself lives. The
   * launcher installs nothing else, and `instance_id` is no longer read by any
   * view: a finished install's outcome is the instance that appears in the list.
   */
  installingVersions: Record<string, TaskInfo>
  /** Install calls whose task the backend has not listed yet, by task id. A
   *  null version means the task was never claimed by this launcher. */
  pendingInstallTasks: Record<string, string | null>
  /** The installable list per host, so switching back to a host does not ask
   *  again; the refresh button is what asks again. */
  remoteVersionsByHost: Record<string, RemoteVersion[]>
  /** Why one host's installable list could not be read. Same rule as
   *  `hostErrors`: no list is served from a failed call. */
  remoteErrors: Record<string, string>
  remoteLoading: boolean
  runtime: RuntimeStatus | null
  healthLogs: HealthLogItem[]
  loaded: boolean
}

export const useLauncherStore = defineStore('launcher', {
  state: (): LauncherState => ({
    hosts: [],
    currentHostId: LOCAL_HOST_ID,
    allHosts: true,
    homesByHost: {},
    versionsByHost: {},
    hostErrors: {},
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
    remoteVersionsByHost: {},
    remoteErrors: {},
    remoteLoading: false,
    runtime: null,
    healthLogs: [],
    loaded: false,
  }),

  getters: {
    /** The machine the Library is showing. */
    currentHost: (s): HostWithStatus | null =>
      s.hosts.find((h) => h.id === s.currentHostId) ?? s.hosts[0] ?? null,
    hostById: (s) => (id?: string | null) => s.hosts.find((h) => h.id === id) ?? null,
    instanceById: (s) => (id: string) => s.instances.find((i) => i.id === id),
    /** The current host's versions — what the Library's first tab shows. */
    versions: (s) => s.versionsByHost[s.currentHostId] ?? [],
    /** The current host's DSH_HOMEs. */
    homes: (s) => s.homesByHost[s.currentHostId] ?? [],
    /** Why the current host's resources could not be read; `''` when they could. */
    hostError: (s) => s.hostErrors[s.currentHostId] ?? '',
    /** The current host's installable versions (cached per host). */
    remoteVersions: (s) => s.remoteVersionsByHost[s.currentHostId] ?? [],
    /** Why the current host's installable list could not be read. */
    remoteError: (s) => s.remoteErrors[s.currentHostId] ?? '',
    /**
     * Look-ups by id search every host's cache: a row on the board may belong
     * to a machine other than the current one, and it still has a name.
     */
    versionById: (s) => (id: string) =>
      Object.values(s.versionsByHost).flat().find((v) => v.id === id),
    homeById: (s) => (id: string) =>
      Object.values(s.homesByHost).flat().find((h) => h.id === id),
    instancesOfHost: (s) => (hostId: string) =>
      s.instances.filter((i) => (i.host_id ?? LOCAL_HOST_ID) === hostId),
    /**
     * The instances the board is scoped to: every host's, or the current
     * host's when 全部 is off. One definition, so the filter, the summary and
     * the bulk actions cannot disagree about what the user is looking at.
     */
    visibleInstances: (s) =>
      s.allHosts
        ? s.instances
        : s.instances.filter((i) => (i.host_id ?? LOCAL_HOST_ID) === s.currentHostId),
    // Referential integrity: "who uses this HOME / version" is answered here,
    // once, instead of being re-derived per view. Ids are unique across hosts,
    // so these need no host of their own.
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

      const [hosts, instances, settings, statuses, runtime, tasks] = await Promise.all([
        api.listHosts(),
        // Every host's instances: the board's 全部 view is the default, and the
        // request says so explicitly (「没说话」 means 本机 everywhere else).
        api.listInstances(null, true),
        api.getSettings(),
        api.listInstanceStatus(),
        api.getRuntimeStatus(),
        // The task list is the install ledger. It is fetched after the rest
        // because a task that just finished is an instance that must be in
        // `this.instances` before its progress row is retired.
        api.listTasks(),
      ])
      this.hosts = hosts
      if (!hosts.some((h) => h.id === this.currentHostId)) {
        this.currentHostId = hosts.find((h) => h.builtin)?.id ?? hosts[0]?.id ?? LOCAL_HOST_ID
      }
      this.instances = instances
      this.settings = settings
      this.statusById = Object.fromEntries(statuses.map((st) => [st.id, st]))
      this.runtime = runtime
      this.installingVersions = installLedger(tasks)
      this.loaded = true
      live = true
      pending.forEach(applyStatus)

      // The current host's resources first — the Library may be on screen — and
      // the other hosts' after, so a board row belonging to another machine can
      // still name its version. A host that cannot be reached records why and
      // keeps no list; nothing here raises a toast for a machine the user is
      // not even looking at.
      await this.loadHostResources(this.currentHostId)
      for (const host of this.hosts) {
        if (host.id !== this.currentHostId) void this.loadHostResources(host.id)
      }

      // Only install progress is tracked, and only while it runs: an install
      // whose event never arrived is recovered by `watchInstallTask` when the
      // Versions page starts one, and one that finished without an event is
      // retired on the next refresh. A running task already in the ledger is
      // overwritten with the event's fuller record.
      await api.onTaskProgress((p) => {
        if (p.state === 'running') {
          // Keyed by version, so adoption needs the version the row is under:
          // from the claim `watchInstallTask` left, or from the task the
          // backend just listed.
          const version = this.pendingInstallTasks[p.id] ?? this.installingVersions[p.id]?.version
          delete this.pendingInstallTasks[p.id]
          if (version == null) {
            // Not ours to show, but the backend knows it by id: keep watching
            // so it lands here if the row ever appears.
            if (this.pendingInstallTasks[p.id] === undefined) this.pendingInstallTasks[p.id] = null
            return
          }
          this.installingVersions[version] = {
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
        }
        // Terminal: it may have created an instance and a version directory.
        // Re-read both, but only when the event names an instance — otherwise
        // every install's progress tick would trigger three full re-reads.
        if (p.instance_id) {
          this.refreshInstances()
          this.refreshVersions()
          this.refreshHomes()
        }
      })

      await api.onTaskLog((l) => {
        const existing = Object.values(this.installingVersions).find((t) => t.id === l.id)
        if (!existing) return
        if (existing.logs.length >= 1000) existing.logs.shift()
        existing.logs.push(l.line)
      })
    },

    /**
     * One host's DSH_HOMEs and installed versions, cached by host. Both are
     * read together because a host that cannot be reached fails both, and one
     * honest error per host (with no list beside it) is what the Library shows.
     */
    async loadHostResources(hostId: string) {
      try {
        const [homes, versions] = await Promise.all([
          api.listHomes(hostId),
          api.listVersions(hostId),
        ])
        this.homesByHost[hostId] = homes
        this.versionsByHost[hostId] = versions
        delete this.hostErrors[hostId]
      } catch (e) {
        // No list is kept for a host that failed: the previous one would be
        // yesterday's answer presented as today's (ADR-0006).
        this.hostErrors[hostId] = String(e)
      }
    },

    /**
     * Sets the one 当前主机 — from the Library's picker or from the instances
     * page's filter, it is the same state either way (ADR-0006 §3). Its
     * resources are read the first time (or after a failed read); switching
     * back and forth does not re-ask a machine that already answered.
     */
    async selectHost(hostId: string) {
      this.currentHostId = hostId
      if (!this.versionsByHost[hostId] || !this.homesByHost[hostId]) {
        await this.loadHostResources(hostId)
      }
      void this.refreshRemoteVersions()
    },

    /** The instances page's 全部 view switch. */
    setAllHosts(value: boolean) {
      this.allHosts = value
    },

    async refreshInstances() {
      this.instances = await api.listInstances(null, true)
    },
    async refreshVersions(hostId?: string) {
      await this.loadHostResources(hostId ?? this.currentHostId)
    },
    async refreshHomes(hostId?: string) {
      await this.loadHostResources(hostId ?? this.currentHostId)
    },
    /** Re-reads the install ledger the Versions page renders. */
    async refreshInstallTasks() {
      this.installingVersions = installLedger(await api.listTasks())
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
        this.installingVersions[version] = task
        return
      }
      this.pendingInstallTasks[taskId] = version
    },

    async checkRuntime(force = false) {
      this.runtime = await api.getRuntimeStatus(force)
    },

    /**
     * The installable versions of the current host: asked of that host's own
     * registry when it has one, and of this Mac's npm when it does not. The
     * answer is cached per host — the refresh button is what asks again.
     */
    async refreshRemoteVersions(force = false) {
      const hostId = this.currentHostId
      if (!force && this.remoteVersionsByHost[hostId]) return
      this.remoteLoading = true
      try {
        const list = await api.fetchAvailableVersions(hostId)
        this.remoteVersionsByHost[hostId] = list
        if (this.currentHostId === hostId) delete this.remoteErrors[hostId]
      } catch (e) {
        // A failed fetch leaves no list behind: the page says why instead of
        // showing the last one it managed to read.
        delete this.remoteVersionsByHost[hostId]
        this.remoteErrors[hostId] = String(e)
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

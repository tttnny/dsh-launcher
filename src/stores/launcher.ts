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

/**
 * The install ledger: versions an install task is currently fetching, keyed by
 * **host + version** — two machines can install the same version at the same
 * time, and each of them shows its own progress on its own row. Accepts the
 * full task list, so both the initial fetch and a re-read build it the same way.
 */
function installLedger(tasks: TaskInfo[]): Record<string, TaskInfo> {
  return Object.fromEntries(
    tasks
      .filter((t) => t.kind === 'install-version' && t.state === 'running')
      .map((t) => [installKey(t.host_id, t.version), t]),
  )
}

/**
 * How one install is identified on the Versions page: the machine, then the
 * version. Neither can contain `:` (a version is semver plus tag characters,
 * a host id is a uuid or the local literal), so one string identifies one
 * install.
 */
export function installKey(
  hostId: string | undefined,
  version: string,
): string {
  return `${hostId ?? LOCAL_HOST_ID}:${version}`
}

/**
 * The event subscriptions `init()` opened, so they can be released again.
 * Module-level rather than store state: they are host handles, not data the
 * views render, and Vue must not proxy them.
 */
let disposers: (() => void)[] = []
let initRun: Promise<void> | null = null

interface LauncherState {
  hosts: HostWithStatus[]
  currentHostId: string
  allHosts: boolean
  homesByHost: Record<string, DshHome[]>
  homeErrors: Record<string, string>
  versionsByHost: Record<string, DshVersion[]>
  /**
   * Why one host's installed versions could not be read (「连不上」 and friends),
   * by host id. 「装了哪些」 is a claim only that machine can make, so a host that
   * failed has **no** list here: showing the previous one would present
   * yesterday's answer as today's.
   */
  versionErrors: Record<string, string>
  instances: DshInstance[]
  settings: LauncherSettings
  statusById: Record<string, InstanceStatus>
  /**
   * Installs in flight, keyed by host + version (`installKey`), so the Versions
   * page shows the progress where the version itself lives — on the machine it
   * is being installed on. A failed install stays here with the reason the
   * backend gave, until the ledger is re-read.
   */
  installingVersions: Record<string, TaskInfo>
  /**
   * Installs this page started, by task id, until the backend lists the task:
   * the claim says which host and version a progress event belongs to. `null`
   * means a task that was never claimed here.
   */
  pendingInstallTasks: Record<
    string,
    { hostId: string; version: string } | null
  >
  remoteVersionsByHost: Record<string, RemoteVersion[]>
  remoteErrors: Record<string, string>
  remoteLoading: boolean
  runtime: RuntimeStatus | null
  loaded: boolean
  /**
   * Why one of the six start-up reads failed, one entry each. A failed read
   * leaves only its own data empty and never stops the rest of start-up; the
   * list is here so the interface can say what was not read instead of
   * presenting an empty shelf as the machine's answer.
   */
  initErrors: string[]
}

export const useLauncherStore = defineStore('launcher', {
  state: (): LauncherState => ({
    hosts: [],
    currentHostId: LOCAL_HOST_ID,
    allHosts: true,
    homesByHost: {},
    homeErrors: {},
    versionsByHost: {},
    versionErrors: {},
    instances: [],
    settings: {
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
    loaded: false,
    initErrors: [],
  }),

  getters: {
    currentHost: (s): HostWithStatus | null =>
      s.hosts.find((h) => h.id === s.currentHostId) ?? s.hosts[0] ?? null,
    hostById: (s) => (id?: string | null) =>
      s.hosts.find((h) => h.id === id) ?? null,
    instanceById: (s) => (id: string) => s.instances.find((i) => i.id === id),
    versions: (s) => s.versionsByHost[s.currentHostId] ?? [],
    homes: (s) => s.homesByHost[s.currentHostId] ?? [],
    homeError: (s) => s.homeErrors[s.currentHostId] ?? '',
    versionError: (s) => s.versionErrors[s.currentHostId] ?? '',
    remoteVersions: (s) => s.remoteVersionsByHost[s.currentHostId] ?? [],
    remoteError: (s) => s.remoteErrors[s.currentHostId] ?? '',
    /**
     * Look-ups by id search every host's cache: a row on the board may belong
     * to a machine other than the current one, and it still has a name.
     */
    versionById: (s) => (id: string) =>
      Object.values(s.versionsByHost)
        .flat()
        .find((v) => v.id === id),
    homeById: (s) => (id: string) =>
      Object.values(s.homesByHost)
        .flat()
        .find((h) => h.id === id),
    instancesOfHost: (s) => (hostId: string) =>
      s.instances.filter((i) => (i.host_id ?? LOCAL_HOST_ID) === hostId),
    visibleInstances: (s) =>
      s.allHosts
        ? s.instances
        : s.instances.filter(
            (i) => (i.host_id ?? LOCAL_HOST_ID) === s.currentHostId,
          ),
    // Referential integrity: "who uses this HOME / version" is answered here,
    // once, instead of being re-derived per view. Ids are unique across hosts,
    // so these need no host of their own.
    instancesOfHome: (s) => (homeId: string) =>
      s.instances.filter((i) => i.home_id === homeId),
    instancesOfVersion: (s) => (versionId: string) =>
      s.instances.filter((i) => i.version_id === versionId),
    isHomeShared: (s) => (homeId: string) =>
      s.instances.some((i) => i.home_id === homeId) &&
      s.instances.filter((i) => i.home_id === homeId).length > 1,
    statusOf:
      (s) =>
      (id: string): InstanceStatus =>
        s.statusById[id] ?? {
          id,
          state: 'stopped',
          url: null,
          profile: null,
          exit_code: null,
        },
  },

  actions: {
    async init(): Promise<void> {
      if (!initRun) {
        initRun = this.startup().catch((e) => {
          this.dispose()
          throw e
        })
      }
      return initRun
    },

    dispose() {
      for (const un of disposers) un()
      disposers = []
      initRun = null
    },

    async startup() {
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
      const unInstanceStatus = await api.onInstanceStatus((st) => {
        if (live) applyStatus(st)
        else pending.push(st)
      })
      disposers.push(unInstanceStatus)

      const [hosts, instances, settings, statuses, runtime, tasks] =
        await Promise.allSettled([
          api.listHosts(),
          api.listInstances(null, true),
          api.getSettings(),
          api.listInstanceStatus(),
          api.getRuntimeStatus(),
          api.listTasks(),
        ])
      // Each read stands on its own: one failing interface leaves only its own
      // data empty and is recorded, while the rest still arrive. Start-up
      // always finishes below, so the status stream is never left buffering.
      const initErrors: string[] = []
      if (hosts.status === 'fulfilled') {
        this.hosts = hosts.value
        if (!hosts.value.some((h) => h.id === this.currentHostId)) {
          this.currentHostId =
            hosts.value.find((h) => h.builtin)?.id ??
            hosts.value[0]?.id ??
            LOCAL_HOST_ID
        }
      } else {
        initErrors.push(`读取主机列表失败：${String(hosts.reason)}`)
      }
      if (instances.status === 'fulfilled') {
        this.instances = instances.value
      } else {
        initErrors.push(`读取实例列表失败：${String(instances.reason)}`)
      }
      if (settings.status === 'fulfilled') {
        this.settings = settings.value
      } else {
        initErrors.push(`读取设置失败：${String(settings.reason)}`)
      }
      if (statuses.status === 'fulfilled') {
        this.statusById = Object.fromEntries(
          statuses.value.map((st) => [st.id, st]),
        )
      } else {
        initErrors.push(`读取实例状态失败：${String(statuses.reason)}`)
      }
      if (runtime.status === 'fulfilled') {
        this.runtime = runtime.value
      } else {
        initErrors.push(`读取运行环境失败：${String(runtime.reason)}`)
      }
      if (tasks.status === 'fulfilled') {
        this.installingVersions = installLedger(tasks.value)
      } else {
        initErrors.push(`读取任务列表失败：${String(tasks.reason)}`)
      }
      this.initErrors = initErrors
      this.loaded = true
      live = true
      pending.forEach(applyStatus)

      await this.loadHostResources(this.currentHostId)
      for (const host of this.hosts) {
        if (host.id !== this.currentHostId) void this.loadHostResources(host.id)
      }

      // Only install progress is tracked, and only until it ends: an install
      // whose event never arrived is recovered by `watchInstallTask` when the
      // Versions page starts one, and one that finished without an event is
      // retired on the next refresh.
      const unTaskProgress = await api.onTaskProgress((p) => {
        const claim = this.pendingInstallTasks[p.id]
        if (claim !== undefined) delete this.pendingInstallTasks[p.id]
        const known = Object.values(this.installingVersions).find(
          (t) => t.id === p.id,
        )
        const hostId = known?.host_id ?? claim?.hostId
        const version = known?.version ?? claim?.version
        if (hostId == null || version == null) {
          // Not ours to show. A running task is kept by id so it lands here if
          // its row ever appears; a finished one has no row to land on, and
          // nothing would ever clear a placeholder left behind for it.
          if (p.state === 'running') {
            if (claim === undefined) this.pendingInstallTasks[p.id] = null
          } else {
            delete this.pendingInstallTasks[p.id]
          }
          // An install this page never saw still creates an instance and a
          // version directory somewhere: the lists have to catch up with it.
          if (p.state !== 'running' && p.instance_id) {
            void this.refreshInstances()
            void this.loadHostResources(this.currentHostId)
          }
          return
        }
        const key = installKey(hostId, version)
        const previous = this.installingVersions[key]
        if (p.state === 'running') {
          this.installingVersions[key] = {
            id: p.id,
            kind: 'install-version',
            label: version,
            version,
            host_id: hostId,
            state: 'running',
            percent: p.percent,
            created_at: previous?.created_at ?? Date.now(),
            message: p.message,
            instance_id: p.instance_id ?? previous?.instance_id ?? null,
            instance_name: previous?.instance_name ?? null,
            logs: previous?.logs ?? [],
          }
          return
        }
        // Terminal. A finished install leaves the row (the version itself is
        // now in the installed list); a failure or a cancellation stays there
        // with the backend's reason, which is the only place the user reads it.
        if (p.state === 'done') {
          delete this.installingVersions[key]
        } else {
          this.installingVersions[key] = {
            ...(previous ?? {
              id: p.id,
              kind: 'install-version',
              label: version,
              version,
              host_id: hostId,
              created_at: Date.now(),
              instance_name: null,
              logs: [],
            }),
            state: p.state,
            percent: p.percent,
            message: p.message,
            instance_id: p.instance_id ?? previous?.instance_id ?? null,
            logs: [],
          }
        }
        void this.loadHostResources(hostId)
        if (p.instance_id) void this.refreshInstances()
      })
      disposers.push(unTaskProgress)

      const unTaskLog = await api.onTaskLog((l) => {
        const existing = Object.values(this.installingVersions).find(
          (t) => t.id === l.id,
        )
        if (!existing) return
        if (existing.logs.length >= 1000) existing.logs.shift()
        existing.logs.push(l.line)
      })
      disposers.push(unTaskLog)
    },

    async loadHostResources(hostId: string) {
      const [homes, versions] = await Promise.allSettled([
        api.listHomes(hostId),
        api.listVersions(hostId),
      ])
      if (homes.status === 'fulfilled') {
        this.homesByHost[hostId] = homes.value
        delete this.homeErrors[hostId]
      } else {
        delete this.homesByHost[hostId]
        this.homeErrors[hostId] = String(homes.reason)
      }
      if (versions.status === 'fulfilled') {
        this.versionsByHost[hostId] = versions.value
        delete this.versionErrors[hostId]
      } else {
        delete this.versionsByHost[hostId]
        this.versionErrors[hostId] = String(versions.reason)
      }
    },

    async selectHost(hostId: string) {
      this.currentHostId = hostId
      if (!this.versionsByHost[hostId] || !this.homesByHost[hostId]) {
        await this.loadHostResources(hostId)
      }
      void this.refreshRemoteVersions()
    },

    setAllHosts(value: boolean) {
      this.allHosts = value
    },

    dropHostCache(hostId: string) {
      delete this.homesByHost[hostId]
      delete this.versionsByHost[hostId]
      delete this.remoteVersionsByHost[hostId]
      delete this.homeErrors[hostId]
      delete this.versionErrors[hostId]
      delete this.remoteErrors[hostId]
    },

    setHosts(hosts: HostWithStatus[]) {
      let currentWentStale = false
      let hostListChanged = hosts.length !== this.hosts.length
      // A machine that left the list takes every answer cached for it — its
      // HOMEs, versions, installable list, the reasons its reads failed — and
      // the states of the instances filed under it: nothing would ever report
      // those again.
      const kept = new Set(hosts.map((h) => h.id))
      for (const before of this.hosts) {
        if (kept.has(before.id)) continue
        this.dropHostCache(before.id)
        for (const inst of this.instances) {
          if ((inst.host_id ?? LOCAL_HOST_ID) === before.id) {
            delete this.statusById[inst.id]
          }
        }
        hostListChanged = true
      }
      for (const host of hosts) {
        const before = this.hosts.find((h) => h.id === host.id)
        if (!before) {
          hostListChanged = true
          continue
        }
        // Anything that decides what that machine answers — where it is, where
        // its data lives, which registry it lists from — makes the cached
        // answers worthless: keeping them would present them as current.
        const changed =
          before.ssh_target !== host.ssh_target ||
          before.data_dir !== host.data_dir ||
          before.registry !== host.registry
        if (!changed) continue
        this.dropHostCache(host.id)
        if (host.id === this.currentHostId) currentWentStale = true
      }
      this.hosts = hosts
      if (!hosts.some((h) => h.id === this.currentHostId)) {
        this.currentHostId =
          hosts.find((h) => h.builtin)?.id ?? hosts[0]?.id ?? LOCAL_HOST_ID
        currentWentStale = true
      }
      if (currentWentStale) void this.loadHostResources(this.currentHostId)
      if (hostListChanged) void this.refreshInstances()
    },

    async refreshInstances() {
      this.instances = await api.listInstances(null, true)
      const ids = new Set(this.instances.map((i) => i.id))
      for (const id of Object.keys(this.statusById)) {
        if (!ids.has(id)) delete this.statusById[id]
      }
    },
    async refreshVersions(hostId?: string) {
      await this.loadHostResources(hostId ?? this.currentHostId)
    },
    async refreshHomes(hostId?: string) {
      await this.loadHostResources(hostId ?? this.currentHostId)
    },
    async refreshInstallTasks() {
      this.installingVersions = installLedger(await api.listTasks())
    },

    /**
     * Follows one install the user just started, so its progress appears on the
     * Versions page before the first task event arrives. `hostId` and `version`
     * are what the row it belongs to is keyed by; the task id is what the start
     * call returned.
     */
    async watchInstallTask(taskId: string, hostId: string, version: string) {
      const tasks = await api.listTasks()
      const task = tasks.find((t) => t.id === taskId)
      if (task) {
        this.installingVersions[installKey(hostId, version)] = task
        return
      }
      this.pendingInstallTasks[taskId] = { hostId, version }
    },

    async checkRuntime(force = false) {
      this.runtime = await api.getRuntimeStatus(force)
    },

    async refreshRemoteVersions(force = false) {
      const hostId = this.currentHostId
      if (!force && this.remoteVersionsByHost[hostId]) return
      this.remoteLoading = true
      try {
        const list = await api.fetchAvailableVersions(hostId)
        this.remoteVersionsByHost[hostId] = list
        if (this.currentHostId === hostId) delete this.remoteErrors[hostId]
      } catch (e) {
        delete this.remoteVersionsByHost[hostId]
        this.remoteErrors[hostId] = String(e)
      } finally {
        this.remoteLoading = false
      }
    },
  },
})

import type {
  ChangeOutcome,
  ConnectionTest,
  DoctorReport,
  DshHome,
  DshInstance,
  DshVersion,
  Host,
  HostInput,
  HostWithStatus,
  InstanceLocation,
  InstanceStatus,
  LauncherSettings,
  LauncherUpdateInfo,
  PluginView,
  RemoteVersion,
  RuntimeStatus,
  SetBundleEnabledInput,
  RemoveBundleInput,
  SshAlias,
  TaskInfo,
  TaskLog,
  TaskProgress,
} from './types'
import type { Listener } from './mock'
import {
  call,
  isTauri,
  subscribeTauriInstanceStatus,
  subscribeTauriTaskProgress,
  subscribeTauriTaskLog,
} from './tauri'

export const api = {
  isTauri,

  getRuntimeStatus: (force = false, hostId?: string | null) =>
    call<RuntimeStatus>('get_runtime_status', {
      force,
      hostId: hostId ?? null,
    }),
  setToolchainOverride: (
    tool: 'node' | 'pnpm',
    path: string | null,
    hostId?: string | null,
  ) =>
    call<RuntimeStatus>('set_toolchain_override', {
      tool,
      path,
      hostId: hostId ?? null,
    }),

  listHosts: () => call<HostWithStatus[]>('list_hosts'),
  createHost: (input: HostInput) => call<Host>('create_host', { input }),
  updateHost: (id: string, input: HostInput) =>
    call<Host>('update_host', { id, input }),
  deleteHost: (id: string) => call<void>('delete_host', { id }),
  testHostConnection: (id: string) =>
    call<ConnectionTest>('test_host_connection', { id }),
  listSshAliases: () => call<SshAlias[]>('list_ssh_aliases'),

  listHomes: (hostId?: string) => call<DshHome[]>('list_homes', { hostId }),
  createHome: (name: string, path: string, hostId?: string) =>
    call<DshHome>('create_home', { name, path, hostId }),
  removeHome: (id: string, hostId?: string) =>
    call<void>('remove_home', { id, hostId }),
  defaultDedicatedHomePath: (name: string, hostId?: string) =>
    call<string>('default_dedicated_home_path', { name, hostId }),

  listVersions: (hostId?: string) =>
    call<DshVersion[]>('list_versions', { hostId }),
  fetchAvailableVersions: (hostId?: string) =>
    call<RemoteVersion[]>('fetch_available_versions', { hostId }),
  removeVersion: (id: string, hostId?: string) =>
    call<void>('remove_version', { id, hostId }),
  setVersionPathLink: (id: string | null, hostId?: string) =>
    call<void>('set_version_path_link', { id, hostId }),
  startInstallVersionTask: (version: string, hostId?: string) =>
    call<string>('start_install_version_task', { version, host_id: hostId }),

  listTasks: () => call<TaskInfo[]>('list_tasks'),
  removeTask: (id: string) => call<void>('remove_task', { id }),
  cancelTask: (id: string) => call<void>('cancel_task', { id }),

  listInstances: (hostId?: string | null, allHosts = false) =>
    call<DshInstance[]>('list_instances', { hostId: hostId ?? null, allHosts }),
  createInstance: (
    name: string,
    versionId: string,
    homeId: string,
    hostId?: string,
  ) =>
    call<DshInstance>('create_instance', { name, versionId, homeId, hostId }),
  deleteInstance: (id: string, hostId?: string | null) =>
    call<void>('delete_instance', { id, hostId: hostId ?? null }),
  updateInstance: (input: DshInstance, hostId?: string | null) =>
    call<DshInstance>('update_instance', { input, hostId: hostId ?? null }),

  listProfiles: (homeId: string) =>
    call<string[]>('list_profiles', { home_id: homeId }),
  createProfile: (homeId: string, name: string) =>
    call<string>('create_profile', { home_id: homeId, name }),
  copyProfile: (homeId: string, source: string, name: string) =>
    call<string>('copy_profile', { home_id: homeId, source, name }),
  renameProfile: (homeId: string, oldName: string, newName: string) =>
    call<string>('rename_profile', {
      home_id: homeId,
      old_name: oldName,
      new_name: newName,
    }),
  deleteProfile: (homeId: string, name: string) =>
    call<void>('delete_profile', { home_id: homeId, name }),
  setInstanceIcon: (instanceId: string, source: string) =>
    call<void>('set_instance_icon', { instanceId, source }),
  clearInstanceIcon: (instanceId: string) =>
    call<void>('clear_instance_icon', { instanceId }),
  readInstanceIcon: (instanceId: string) =>
    call<string | null>('read_instance_icon', { instanceId }),
  pendingDeepLink: () => call<string | null>('pending_deep_link'),

  startInstance: (id: string, profile: string) =>
    call<void>('start_instance', { id, profile }),
  checkInstanceHealth: (instanceId: string, profile: string) =>
    call<DoctorReport>('check_instance_health', {
      instance_id: instanceId,
      profile,
    }),
  stopInstance: (id: string) => call<void>('stop_instance', { id }),
  restartInstance: (id: string) => call<void>('restart_instance', { id }),
  openInstanceWindow: (id: string) =>
    call<void>('open_instance_window', { id }),
  openExternal: (url: string) => call<void>('open_external', { url }),
  listInstanceStatus: () => call<InstanceStatus[]>('list_instance_status'),
  refreshInstanceStatus: (id: string) =>
    call<void>('refresh_instance_status', { id }),

  getSettings: () => call<LauncherSettings>('get_settings'),
  updateSettings: (settings: Partial<LauncherSettings>) =>
    call<LauncherSettings>('update_settings', { settings }),

  checkLauncherUpdate: () => call<LauncherUpdateInfo>('check_launcher_update'),
  async downloadLauncherUpdate(version: string, onProgress: Listener<number>) {
    if (!isTauri) {
      for (const percent of [20, 45, 70, 99]) {
        await new Promise((resolve) => setTimeout(resolve, 250))
        onProgress(percent)
      }
      return
    }
    const { Channel } = await import('@tauri-apps/api/core')
    await call<void>('download_launcher_update', {
      version,
      onProgress: new Channel<number>(onProgress),
    })
  },
  getLauncherDirectory: () => call<string>('get_launcher_directory'),
  openLauncherDirectory: () => call<string>('open_launcher_directory'),
  openLauncherLog: () => call<string>('open_launcher_log'),
  openInstanceLog: (instanceId: string) =>
    call<string>('open_instance_log', { instanceId }),
  openInstanceDirectory: (instanceId: string) =>
    call<string>('open_instance_directory', { instanceId }),
  instanceRemoteLocation: (instanceId: string) =>
    call<InstanceLocation>('instance_remote_location', { instanceId }),
  hostDirectoryCommand: (hostId: string, path: string) =>
    call<string>('host_directory_command', { hostId, path }),
  openHomeDirectory: (homeId: string) =>
    call<string>('open_home_directory', { homeId }),
  async getLauncherVersion(): Promise<string> {
    if (isTauri) {
      const { getVersion } = await import('@tauri-apps/api/app')
      return getVersion()
    }
    return '0.0.1-rc.1'
  },

  listPlugins: (homeId: string, profile: string, instanceId?: string) =>
    call<PluginView>('list_plugins', {
      home_id: homeId,
      profile,
      instance_id: instanceId ?? null,
    }),
  setBundleEnabled: (input: SetBundleEnabledInput) =>
    call<ChangeOutcome>('set_bundle_enabled', { input }),
  removeBundle: (input: RemoveBundleInput) =>
    call<ChangeOutcome>('remove_bundle', { input }),
  openInstanceTerminal: (instanceId: string) =>
    call<string>('open_instance_terminal', { instanceId }),

  async onInstanceStatus(cb: Listener<InstanceStatus>): Promise<() => void> {
    if (isTauri) return subscribeTauriInstanceStatus(cb)
    if (!import.meta.env.DEV) throw new Error('浏览器预览仅在开发模式可用')
    const { subscribeMockInstanceStatus } = await import('./mock')
    return subscribeMockInstanceStatus(cb)
  },

  async onTaskProgress(cb: Listener<TaskProgress>): Promise<() => void> {
    if (isTauri) return subscribeTauriTaskProgress(cb)
    if (!import.meta.env.DEV) throw new Error('浏览器预览仅在开发模式可用')
    const { subscribeMockTaskProgress } = await import('./mock')
    return subscribeMockTaskProgress(cb)
  },

  async onTaskLog(cb: Listener<TaskLog>): Promise<() => void> {
    if (isTauri) return subscribeTauriTaskLog(cb)
    if (!import.meta.env.DEV) throw new Error('浏览器预览仅在开发模式可用')
    const { subscribeMockTaskLog } = await import('./mock')
    return subscribeMockTaskLog(cb)
  },
}

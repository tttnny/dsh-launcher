// The public api interface: one object every view/store imports. Behind it
// sit two adapters — the Tauri bridge (desktop) and the localStorage mock
// (browser preview) — selected in tauri.ts; see mock.ts for the contract
// test note.

import type {
  DoctorReport,
  DshHome,
  DshInstance,
  DshVersion,
  InstalledPlugin,
  InstanceStatus,
  LauncherSettings,
  LauncherUpdateInfo,
  RemoteVersion,
  RuntimeStatus,
  SetPluginsEnabledInput,
  TaskInfo,
  TaskLog,
  TaskProgress,
  UninstallPluginInput,
} from './types'
import type { Listener } from './mock'
import {
  call,
  isTauri,
  subscribeTauriInstanceStatus,
  subscribeTauriTaskProgress,
  subscribeTauriTaskLog,
} from './tauri'
import {
  subscribeMockInstanceStatus,
  subscribeMockTaskProgress,
  subscribeMockTaskLog,
} from './mock'

// ---------------------------------------------------------------------------
// Public API — the interface. Transport is selected once in tauri.ts.
// ---------------------------------------------------------------------------

export const api = {
  isTauri,

  getRuntimeStatus: () => call<RuntimeStatus>('get_runtime_status'),

  listHomes: () => call<DshHome[]>('list_homes'),
  createHome: (name: string, path: string) => call<DshHome>('create_home', { name, path }),
  removeHome: (id: string) => call<void>('remove_home', { id }),
  defaultDedicatedHomePath: (name: string) => call<string>('default_dedicated_home_path', { name }),

  listVersions: () => call<DshVersion[]>('list_versions'),
  fetchAvailableVersions: () => call<RemoteVersion[]>('fetch_available_versions'),
  removeVersion: (id: string) => call<void>('remove_version', { id }),
  /** Downloads & installs a DSH version, auto-creates dedicated HOME, and registers the 1:1 instance. */
  startInstallVersionTask: (version: string) =>
    call<string>('start_install_version_task', { version }),

  listTasks: () => call<TaskInfo[]>('list_tasks'),
  removeTask: (id: string) => call<void>('remove_task', { id }),
  cancelTask: (id: string) => call<void>('cancel_task', { id }),

  listInstances: () => call<DshInstance[]>('list_instances'),
  updateInstance: (input: DshInstance) => call<DshInstance>('update_instance', { input }),
  /** Sets the instance's web port; null or out-of-range = random port. */
  setInstancePort: (instanceId: string, port: number | null) =>
    call<DshInstance>('set_instance_port', { instance_id: instanceId, port }),

  listProfiles: (homeId: string) => call<string[]>('list_profiles', { home_id: homeId }),
  createProfile: (homeId: string, name: string) =>
    call<string>('create_profile', { home_id: homeId, name }),
  copyProfile: (homeId: string, source: string, name: string) =>
    call<string>('copy_profile', { home_id: homeId, source, name }),
  renameProfile: (homeId: string, oldName: string, newName: string) =>
    call<string>('rename_profile', { home_id: homeId, old_name: oldName, new_name: newName }),
  deleteProfile: (homeId: string, name: string) =>
    call<void>('delete_profile', { home_id: homeId, name }),
  /** Sets an instance icon from a local image path or http(s) URL. */
  setInstanceIcon: (instanceId: string, source: string) =>
    call<void>('set_instance_icon', { instanceId, source }),
  /** Restores the launcher default icon for an instance. */
  clearInstanceIcon: (instanceId: string) => call<void>('clear_instance_icon', { instanceId }),
  /** Resolves the displayable icon (URL or data URL); null = launcher default. */
  readInstanceIcon: (instanceId: string) => call<string | null>('read_instance_icon', { instanceId }),
  /** Cold-start deep link from process argv (null when launched normally). */
  pendingDeepLink: () => call<string | null>('pending_deep_link'),

  startInstance: (id: string, profile: string) => call<void>('start_instance', { id, profile }),
  checkInstanceHealth: (instanceId: string, profile: string) =>
    call<DoctorReport>('check_instance_health', { instance_id: instanceId, profile }),
  stopInstance: (id: string) => call<void>('stop_instance', { id }),
  /** One backend lifecycle transition: stop + start on the running profile. */
  restartInstance: (id: string) => call<void>('restart_instance', { id }),
  openInstanceWindow: (id: string) => call<void>('open_instance_window', { id }),
  /** Opens an external http(s) URL in the system browser. */
  openExternal: (url: string) => call<void>('open_external', { url }),
  listInstanceStatus: () => call<InstanceStatus[]>('list_instance_status'),

  getSettings: () => call<LauncherSettings>('get_settings'),
  updateSettings: (settings: Partial<LauncherSettings>) => call<LauncherSettings>('update_settings', { settings }),

  /** Starts the one-click Node.js install background task (issue #23). */
  startInstallNodeTask: () => call<string>('start_install_node_task'),
  /** Checks GitHub for a newer launcher release on the given channel. */
  checkLauncherUpdate: (channel: 'dev' | 'release' = 'dev') =>
    call<LauncherUpdateInfo>('check_launcher_update', { channel }),
  /** The launcher's own data directory (shown next to the open button). */
  getLauncherDirectory: () => call<string>('get_launcher_directory'),
  /** Opens the launcher data directory in the system file manager. */
  openLauncherDirectory: () => call<string>('open_launcher_directory'),
  /** Reveals the launcher runtime log (latest.log) with the file selected. */
  openLauncherLog: () => call<string>('open_launcher_log'),
  /** Reveals one instance's runtime log with the file selected. */
  openInstanceLog: (instanceId: string) => call<string>('open_instance_log', { instanceId }),
  /** Opens an instance's DSH_HOME directory in the file manager. */
  openInstanceDirectory: (instanceId: string) =>
    call<string>('open_instance_directory', { instanceId }),
  /** Opens a DSH_HOME directory in the file manager. */
  openHomeDirectory: (homeId: string) =>
    call<string>('open_home_directory', { homeId }),
  /** The running launcher's own version (stamped at build time by CI). */
  async getLauncherVersion(): Promise<string> {
    if (isTauri) {
      const { getVersion } = await import('@tauri-apps/api/app')
      return getVersion()
    }
    return '0.2.0-dev.1'
  },

  // Profile plugins (scoped by HOME + profile): list / enable / disable / uninstall.
  listInstalledPlugins: (homeId: string, profile: string) =>
    call<InstalledPlugin[]>('list_installed_plugins', { home_id: homeId, profile }),
  setPluginsEnabled: (input: SetPluginsEnabledInput) => call<void>('set_plugins_enabled', { input }),
  uninstallPlugin: (input: UninstallPluginInput) => call<void>('uninstall_plugin', { input }),

  // External terminal: opens Terminal.app / Ghostty for one instance.
  openInstanceTerminal: (instanceId: string) =>
    call<string>('open_instance_terminal', { instanceId }),


  async onInstanceStatus(cb: Listener<InstanceStatus>): Promise<() => void> {
    return isTauri ? subscribeTauriInstanceStatus(cb) : Promise.resolve(subscribeMockInstanceStatus(cb))
  },

  async onTaskProgress(cb: Listener<TaskProgress>): Promise<() => void> {
    return isTauri ? subscribeTauriTaskProgress(cb) : Promise.resolve(subscribeMockTaskProgress(cb))
  },

  async onTaskLog(cb: Listener<TaskLog>): Promise<() => void> {
    return isTauri ? subscribeTauriTaskLog(cb) : Promise.resolve(subscribeMockTaskLog(cb))
  },
}

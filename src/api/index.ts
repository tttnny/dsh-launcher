// The public api interface: one object every view/store imports. Behind it
// sit two adapters — the Tauri bridge (desktop) and the localStorage mock
// (browser preview) — selected in tauri.ts; see mock.ts for the contract
// test note.

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

  /** `force` 为 true 时忽略缓存，重新探测一次（供「重新检测」按钮使用）。
   *  `hostId` 缺省是本机；每台主机各有一份结论与一份用户指定。 */
  getRuntimeStatus: (force = false, hostId?: string | null) =>
    call<RuntimeStatus>('get_runtime_status', { force, hostId: hostId ?? null }),
  /** 指定某台主机上某个工具的可执行文件；path 为 null 表示取消指定，回到自动检测。
   *  返回那台主机刷新后的状态。 */
  setToolchainOverride: (tool: 'node' | 'pnpm', path: string | null, hostId?: string | null) =>
    call<RuntimeStatus>('set_toolchain_override', { tool, path, hostId: hostId ?? null }),

  /**
   * The machines the launcher manages, each with the status last observed for
   * it. 本机 is always one of them and never has a status: it is not a machine
   * to reach (ADR-0006).
   */
  listHosts: () => call<HostWithStatus[]>('list_hosts'),
  /** Adds a remote host. Nothing is dialed: a machine that is off can be saved. */
  createHost: (input: HostInput) => call<Host>('create_host', { input }),
  /** Rewrites one host's editable fields; its id and 本机-ness are the record's. */
  updateHost: (id: string, input: HostInput) => call<Host>('update_host', { id, input }),
  /**
   * Removes a host and the records filed under it — the launcher's own records
   * only. Nothing on that machine is touched and the instances running there
   * keep running (ADR-0006 §5, ADR-0007).
   */
  deleteHost: (id: string) => call<void>('delete_host', { id }),
  /**
   * Tests one host: its system, home, data directory and what Node / pnpm it
   * resolves to. A machine that cannot be reached comes back as a result
   * (「不可达」 plus one of the four classes), not as a thrown error.
   */
  testHostConnection: (id: string) => call<ConnectionTest>('test_host_connection', { id }),
  /** The aliases in the user's own `~/.ssh/config`, for the add-host picker. */
  listSshAliases: () => call<SshAlias[]>('list_ssh_aliases'),

  listHomes: (hostId?: string) => call<DshHome[]>('list_homes', { hostId }),
  createHome: (name: string, path: string, hostId?: string) =>
    call<DshHome>('create_home', { name, path, hostId }),
  removeHome: (id: string, hostId?: string) => call<void>('remove_home', { id, hostId }),
  defaultDedicatedHomePath: (name: string, hostId?: string) =>
    call<string>('default_dedicated_home_path', { name, hostId }),

  /** One host's installed versions, with each version's path and bin readiness. */
  listVersions: (hostId?: string) => call<DshVersion[]>('list_versions', { hostId }),
  /**
   * The versions installable on one host: asked of that host's own npm when it
   * names a registry, and of this Mac's when it does not (ADR-0006 故事 13).
   */
  fetchAvailableVersions: (hostId?: string) =>
    call<RemoteVersion[]>('fetch_available_versions', { hostId }),
  removeVersion: (id: string, hostId?: string) => call<void>('remove_version', { id, hostId }),
  /**
   * Downloads & installs a DSH version **on one host**, auto-creates dedicated
   * HOME, and registers the 1:1 instance. `hostId` absent means 本机.
   *
   * The key sent is `host_id`, because that is the name this command reads:
   * it declares `rename_all = "snake_case"` (the rule for commands with
   * underscore parameters), and Tauri looks an argument up by exactly that
   * key. A camelCase key is not an error there — it is read as 「absent」, so
   * the install would go to 本机 while the page names another machine.
   */
  startInstallVersionTask: (version: string, hostId?: string) =>
    call<string>('start_install_version_task', { version, host_id: hostId }),

  listTasks: () => call<TaskInfo[]>('list_tasks'),
  removeTask: (id: string) => call<void>('remove_task', { id }),
  cancelTask: (id: string) => call<void>('cancel_task', { id }),

  /**
   * Instances of one host, or of **every** host when the caller says so
   * (`allHosts`, the board's 全部 view). `hostId` absent and `allHosts` false
   * means 本机.
   */
  listInstances: (hostId?: string | null, allHosts = false) =>
    call<DshInstance[]>('list_instances', { hostId: hostId ?? null, allHosts }),
  /** Binds an already-installed version to a DSH_HOME as a new instance. */
  createInstance: (name: string, versionId: string, homeId: string, hostId?: string) =>
    call<DshInstance>('create_instance', { name, versionId, homeId, hostId }),
  /** Removes one instance record (stops it first); version and HOME stay. */
  deleteInstance: (id: string, hostId?: string | null) =>
    call<void>('delete_instance', { id, hostId: hostId ?? null }),
  updateInstance: (input: DshInstance, hostId?: string | null) =>
    call<DshInstance>('update_instance', { input, hostId: hostId ?? null }),
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
  /**
   * The page's one retry: ask the instance's own machine again what state it is
   * in. It is what turns 「主机不可达」 back into an answer, and it is why that
   * state never has to be guessed at (ADR-0007).
   */
  refreshInstanceStatus: (id: string) => call<void>('refresh_instance_status', { id }),

  getSettings: () => call<LauncherSettings>('get_settings'),
  updateSettings: (settings: Partial<LauncherSettings>) => call<LauncherSettings>('update_settings', { settings }),

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
  /**
   * The instance's own plugin list (native plugin management, ADR-0001).
   * `instanceId` is the instance the page was opened from: it decides which
   * process answers when two instances run this HOME + profile at once.
   */
  listPlugins: (homeId: string, profile: string, instanceId?: string) =>
    call<PluginView>('list_plugins', {
      home_id: homeId,
      profile,
      instance_id: instanceId ?? null,
    }),
  /**
   * Enables/disables one whole Bundle（补丁层） through the running instance's
   * native plugin manager. The launcher never writes the profile manifest
   * (ADR-0001); a refusal comes back as a ChangeOutcome, not a thrown error.
   */
  setBundleEnabled: (input: SetBundleEnabledInput) => call<ChangeOutcome>('set_bundle_enabled', { input }),
  /**
   * Removes one whole Bundle（补丁层） through the instance's native plugin
   * manager — the same path the Web UI uses. A bundle that ships with DSH
   * (`removable:false`) is refused as an outcome carrying the raw `errorCode`
   * and the localized reason in `diagnostic`, never a thrown error; the
   * launcher never writes the profile files (ADR-0001).
   */
  removeBundle: (input: RemoveBundleInput) => call<ChangeOutcome>('remove_bundle', { input }),
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

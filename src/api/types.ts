// Shared types mirrored from the Rust backend (src-tauri/src/config.rs).

export interface DshHome {
  id: string
  name: string
  path: string
}

export interface DshVersion {
  id: string
  version: string
  dir: string
  /**
   * The version's 专属 DSH_HOME (`<数据目录>/homes/<版本名>`), derived
   * backend-side so the delete-confirmation dialog can name the exact path it
   * will remove. Deleting a version removes this path — and only this one.
   */
  dedicated_home: string
}

export interface DshInstance {
  id: string
  name: string
  version_id: string
  home_id: string
  env_overrides: Record<string, string>
  default_profile: string | null
  last_profile: string | null
  /** http(s) URL, "local" (cropped PNG in the HOME), or null/undefined = launcher default. */
  icon?: string | null
  /** Preferred web port (1-65535); null/undefined = random free port. */
  port?: number | null
  /**
   * Pass --preserve-symlinks to this instance's DSH process. Only needed when
   * the profile depends on a `link:`-ed plugin; off by default because it
   * breaks DSH's settings writes (the Web UI wedges on the first-run notice).
   */
  preserve_symlinks?: boolean
}

export interface LauncherSettings {
  locale: string
  minimize_to_tray: boolean
  autostart: boolean
  last_instance_id: string | null
  theme: ThemeMode
  log_level: LogLevel
  /** External terminal: "system" (Terminal.app) or "ghostty". */
  terminal: string
  /** Route the launcher's own HTTP requests through a proxy. */
  proxy_enabled: boolean
  /** Proxy URL without port (PROXY_URL), e.g. http://127.0.0.1 */
  proxy_url: string
  /** PROXY_PORT */
  proxy_port: number
  /** Comma-separated bypass list (NO_PROXY). */
  no_proxy: string
  /** Inject the proxy into launched dsh instances (overrides instance env; applies on next start). */
  proxy_apply_dsh: boolean
}

/** UI theme: explicit light/dark, or follow the OS color scheme. */
export type ThemeMode = 'light' | 'dark' | 'system'

/** Runtime log level written to <data_dir>/logs/latest.log. */
export type LogLevel = 'debug' | 'info' | 'warn' | 'error'

/** Severity of a dependency-tree preflight finding. */
export type FindingLevel = 'warn' | 'error'

export interface DoctorFinding {
  level: FindingLevel
  /** core-version-mismatch | core-missing | profile-core-copy | profile-core-mixed */
  code: string
  message: string
}

/** Dependency-tree preflight result for an instance + profile (advisory). */
export interface DoctorReport {
  instance_id: string
  profile: string
  findings: DoctorFinding[]
}

/** Result of checking GitHub for a newer launcher release. */
export interface LauncherUpdateInfo {
  current: string
  /** "dev" for -dev.N builds, otherwise "stable". */
  channel: 'dev' | 'stable'
  up_to_date: boolean
  latest: string | null
  url: string | null
  published_at: string | null
}

export type InstanceState = 'stopped' | 'starting' | 'running' | 'exited'

export interface InstanceStatus {
  id: string
  state: InstanceState
  url: string | null
  profile: string | null
  exit_code: number | null
}

export interface ToolStatus {
  installed: boolean
  version: string | null
  path: string | null
}

export interface RuntimeStatus {
  node: ToolStatus
  pnpm: ToolStatus
  /** Detected Node version managers ('nvm' | 'fnm' | 'brew'), most preferred
   *  first, so the setup page can offer a command that fits this machine. */
  managers: string[]
  /** node is present but older than the recommended major (advisory only). */
  node_below_recommended: boolean
  /** The version floors the backend enforces, so UI copy states the same
   *  numbers the resolver checks rather than repeating them as literals. */
  requirements: ToolchainRequirements
}

export interface ToolchainRequirements {
  min_pnpm_major: number
  recommended_node_major: number
  pnpm_install_command: string
  /** Node install command for this machine, chosen from the detected managers. */
  node_install_command: string
}

export type TaskState = 'running' | 'done' | 'error' | 'cancelled'

export interface TaskInfo {
  id: string
  kind: string
  label: string
  version: string
  state: TaskState
  percent: number
  created_at: number
  message: string | null
  instance_id: string | null
  instance_name: string | null
  logs: string[]
}

export interface TaskProgress {
  id: string
  state: TaskState
  percent: number
  message: string | null
  instance_id: string | null
}

export interface TaskLog {
  id: string
  line: string
}

export interface RemoteVersion {
  version: string
  released_at: string | null
  /** 'github' = GitHub-only tag, installed by building from source. */
  source?: 'npm' | 'github' | null
}

// ---------------------------------------------------------------------------
// Native plugin management (frozen contract — design §4; T2–T5 consume, don't
// rename). Values come from the running instance's own plugin manager over
// /api/pluginManager/*; the launcher never reads the profile files.
// ---------------------------------------------------------------------------

/** How the Plugins page may treat the snapshot: managed, or a reason it can't. */
export type PluginViewMode = 'managed' | 'not-running' | 'adopted' | 'unreachable'

/** A native plugin entry (the stable subset of PluginInfo). */
export interface PluginItem {
  /** Row key; the `id` argument of `setPluginEnabled`. */
  entryId: string
  moduleName: string
  /** From `meta.title`; the UI falls back to `moduleName` when absent. */
  title?: string | null
  description?: string | null
  enabled: boolean
  fiberPhase?: string | null
  patchId?: string | null
  /** 'management-required' | 'unaddressable'. */
  readOnlyReason?: string | null
  /**
   * The version of the bundle（补丁层）this entry arrived in. The native
   * inventory has no version for an entry, so the backend fills this in from
   * the bundle providing the entry's package; entries no bundle claims have
   * none.
   */
  version?: string | null
}

/** A Bundle（补丁层）. */
export interface BundleItem {
  /** The `name` argument of `setBundleEnabled` / `removeBundle`. */
  name: string
  title?: string | null
  description?: string | null
  version?: string | null
  enabled: boolean
  installed: boolean
  /** false = can only be disabled, never removed. */
  removable: boolean
  optional: boolean
  /** 'installation' | 'profile' | 'missing'. */
  availability?: string | null
  readOnlyReason?: string | null
  errorCode?: string | null
}

/** One snapshot of the Plugins page; `mode` picks managed vs read-only. */
export interface PluginView {
  mode: PluginViewMode
  /** Read-only explanation, already localized by the backend; shown verbatim. */
  reason?: string | null
  /**
   * True when the list was read from the profile's own files (instance not
   * running) instead of answered by the instance. The offline list is a
   * best-effort, scope-wide projection, so the UI says so.
   */
  files: boolean
  plugins: PluginItem[]
  bundles: BundleItem[]
}

/** A write result, mirroring the native ChangeResult. */
export interface ChangeOutcome {
  changed: boolean
  /** applied | restart-required | overridden | failed | cancelled. */
  application: string
  /** enable | remove. */
  stage: string
  target: string
  /** The raw native `error.code` for a refusal; logic, not display. */
  errorCode?: string | null
  /** The localized refusal reason; the UI shows this verbatim. */
  diagnostic?: string | null
  warnings: string[]
}

export interface SetPluginEnabledInput {
  homeId: string
  profile: string
  id: string
  enabled: boolean
}

export interface SetBundleEnabledInput {
  homeId: string
  profile: string
  name: string
  enabled: boolean
}

export interface RemoveBundleInput {
  homeId: string
  profile: string
  name: string
}

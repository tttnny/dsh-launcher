// Shared types mirrored from the Rust backend (src-tauri/src/config.rs).

/** The built-in local host's id; the literal the backend migrates records to. */
export const LOCAL_HOST_ID = 'host-local'

/**
 * One machine the launcher manages. 本机 is the built-in one (`builtin`);
 * every other host is reached over ssh. A host owns its versions, DSH_HOMEs,
 * profiles and instances (ADR-0006).
 */
export interface Host {
  id: string
  /** The name shown in the UI. The built-in host's is 本机. */
  display_name: string
  /** The ssh target handed to the system ssh verbatim; empty for 本机. */
  ssh_target: string
  /** The npm registry this host's versions are listed from, when it has one. */
  registry?: string | null
  /** Environment variables injected into everything the launcher runs there. */
  extra_env?: Record<string, string>
  /** The launcher's data directory on that host; null for 本机 (its own). */
  data_dir?: string | null
  /** Built-in hosts cannot be deleted: true for 本机 only. */
  builtin: boolean
}

/** 主机当前的状态：连接中 / 可达 / 不可达。 */
export type HostState = 'connecting' | 'reachable' | 'unreachable'

/**
 * What the launcher last learned about reaching one host. Runtime only: it is
 * never written to `config.json`, because 「现在连不上」 is a fact about the
 * network and the moment, not part of the record (ADR-0006).
 */
export interface HostStatus {
  state: HostState
  /** The readable reason, when `unreachable` — one of the four classes. */
  message?: string | null
}

/** A host record plus the status last observed for it (`list_hosts`). */
export interface HostWithStatus extends Host {
  /** `null` = 还没测过, which is not the same as 不可达. */
  status: HostStatus | null
}

/** The editable half of a host record — what the add/edit sheet holds. */
export interface HostInput {
  display_name: string
  ssh_target: string
  registry?: string | null
  extra_env?: Record<string, string>
  data_dir?: string | null
}

/** The four ways a remote call fails, as the page tells them apart. */
export type HostErrorKind =
  'unreachable' | 'auth' | 'timeout' | 'remote_command'

/** One probe of one tool on a remote machine: the path it resolves to, and its version. */
export interface RemoteTool {
  path: string
  version: string
}

/**
 * What 「测试连接」 found on a host. A failure is a **result**, not a thrown
 * error: a machine that is switched off right now can still be saved, and the
 * page says 「不可达」 (ADR-0006).
 */
export interface ConnectionTest {
  ok: boolean
  kind: HostErrorKind | null
  message: string | null
  system: { os: string; release: string; arch: string } | null
  home: string | null
  /**
   * The launcher's data directory on that machine, settled there (`~` expanded,
   * one directory typed two ways is one path). `null` when the value the user
   * typed could not be settled — `data_dir_problem` says why.
   */
  data_dir: string | null
  /** Why `data_dir` could not be settled; the record keeps what the user typed. */
  data_dir_problem: string | null
  /** A fresh host has no data directory yet; the launcher creates it on first write. */
  data_dir_exists: boolean
  /** `null` = 那台机器上没有这个命令。 */
  node: RemoteTool | null
  pnpm: RemoteTool | null
}

/** One alias from the user's own `~/.ssh/config`, for the add-host picker. */
export interface SshAlias {
  alias: string
  host_name: string | null
  user: string | null
  port: number | null
}

export interface DshHome {
  id: string
  name: string
  path: string
  /** The host this DSH_HOME lives on. */
  host_id?: string
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
  /** The host this version is installed on. */
  host_id?: string
  /**
   * Whether that version's bin is there and not empty. Only the machine that
   * owns the directory can answer this, so a remote host reports it over ssh: a
   * directory an install stopped inside is a record to see and delete, not a
   * version anyone can start.
   */
  bin_ready: boolean
  /**
   * Whether this Mac's `~/.local/bin/dsh` runs this version. Only 本机 can say
   * so: the link is on this machine, and a version on a server is not what
   * magpie here would run.
   */
  path_linked: boolean
}

export interface DshInstance {
  id: string
  name: string
  version_id: string
  home_id: string
  /** The host this instance runs on. */
  host_id?: string
  env_overrides: Record<string, string>
  default_profile: string | null
  last_profile: string | null
  /** http(s) URL, "local" (cropped PNG in the HOME), or null/undefined = launcher default. */
  icon?: string | null
  /** Preferred web port (1-65535); null/undefined = random free port. */
  port?: number | null
}

/**
 * Where an instance on another machine lives, and how a user reaches it from
 * this Mac: the path to paste, and one command that runs there. The instance
 * card offers these instead of 「打开目录」, which can only open a directory on
 * this Mac (ADR-0006).
 */
export interface InstanceLocation {
  host_id: string
  host_name: string
  /** The ssh target exactly as the host record spells it. */
  ssh_target: string
  /** The DSH_HOME as that machine spells it. */
  path: string
  /** `ls -la` at that path, through ssh — built by the backend, which owns the quoting. */
  command: string
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

/**
 * What an instance's process is doing, as the launcher can tell right now.
 *
 * `unreachable` is not a relabelling of `stopped`: it means the machine that
 * runs the instance did not answer, so nothing is known about the process. The
 * two must stay apart — reading 「主机不可达」 as 「已停止」 is what makes a user
 * press 启动 and end up with a second DSH on a server that was running one all
 * along (ADR-0007).
 */
export type InstanceState =
  'stopped' | 'starting' | 'running' | 'exited' | 'unreachable'

export interface InstanceStatus {
  id: string
  state: InstanceState
  /** Where the instance can be opened **from this Mac**: for a remote instance
   * the forwarded loopback address, not the one DSH printed on its own machine
   * (ADR-0008). */
  url: string | null
  /** Where a remote instance really is: the address DSH printed on its own
   * machine. `null` for 本机's instances. Stated separately from `url` because
   * the two differ whenever the remote port was taken here. */
  remote_url?: string | null
  profile: string | null
  exit_code: number | null
}

/** Where the executable the launcher picked came from. */
export type ToolSource = 'manual' | 'system' | 'fallback'

export interface ToolStatus {
  installed: boolean
  version: string | null
  path: string | null
  /** 被选中的可执行文件从哪来；未安装时为 null。 */
  source: ToolSource | null
  /** 用户指定过的路径（即使已失效也如实带回）。 */
  manual_path: string | null
}

/** 探测路上被跳过的候选（远程主机才报）：低于门槛的 Node / pnpm、读不出版本号的、
 *  以及失效的手动指定。`reason` 是代码不是句子，页面按语言渲染。 */
export interface SkippedCandidate {
  tool: 'node' | 'pnpm'
  path: string
  version?: string
  /** 'below-minimum' | 'unparsable' | 'unusable' */
  reason: string
  /** 候选报出来的大版本号（低于门槛时才有）。 */
  major?: number
}

export interface RuntimeStatus {
  /** 这份结论是哪台主机的（本机是 LOCAL_HOST_ID）。页面切换主机后据此丢弃迟到的答案。 */
  host_id: string
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
  /** 系统环境变量是从哪个 shell 读来的（如 '/bin/zsh'，远程是那台机器的 bash）；
   *  null = 读 shell 失败，退回应用继承的 PATH。 */
  system_env_shell: string | null
  /** 被跳过的候选，附原因。本机不跳过任何候选（老 Node 只告警），所以这里是空的。 */
  skipped_candidates: SkippedCandidate[]
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
  /**
   * The machine this task works on. Two hosts can install the same version at
   * the same time, so the Versions page keys an install's row by host +
   * version — this is the host half of that.
   */
  host_id: string
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
export type PluginViewMode =
  'managed' | 'not-running' | 'adopted' | 'unreachable'

/**
 * A Bundle（补丁层）— the unit this page manages: what a profile selects and
 * what DSH mounts as one patch layer. The rows inside it are DSH's own
 * composition detail, reported as a count rather than a control surface.
 */
export interface BundleItem {
  /** The `name` argument of `setBundleEnabled` / `removeBundle`. */
  name: string
  /** How many composition-tree rows this layer contributes. */
  rowCount: number
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

/**
 * The instance an action was taken from. One DSH_HOME and one profile can be
 * running in two instances at once, so it ranks the candidates: the instance
 * the user came from wins when it is itself running that profile, and the first
 * registered match is only the fallback.
 */
export interface InstanceScope {
  /** The instance the page was opened from; `null` when it is unknown. */
  instanceId?: string | null
}

/** One snapshot of the Plugins page; `mode` picks managed vs read-only. */
export interface PluginView {
  mode: PluginViewMode
  /**
   * The instance that answered. The page names this one rather than the one it
   * was opened from: the two can differ when the row's instance stopped (or
   * when the preference did not resolve) and another one runs the same profile.
   * `null` in every read-only mode.
   */
  instanceId?: string | null
  /** Read-only explanation, already localized by the backend; shown verbatim. */
  reason?: string | null
  /**
   * The instance's own bundle list. Empty in every read-only mode: the launcher
   * does not guess a layer list from files, so without a running instance there
   * is nothing to show.
   */
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

export interface SetBundleEnabledInput extends InstanceScope {
  homeId: string
  profile: string
  name: string
  enabled: boolean
}

export interface RemoveBundleInput extends InstanceScope {
  homeId: string
  profile: string
  name: string
}

export const LOCAL_HOST_ID = 'host-local'

export interface Host {
  id: string
  display_name: string
  ssh_target: string
  registry?: string | null
  extra_env?: Record<string, string>
  data_dir?: string | null
  builtin: boolean
}

export type HostState = 'connecting' | 'reachable' | 'unreachable'

export interface HostStatus {
  state: HostState
  message?: string | null
}

export interface HostWithStatus extends Host {
  status: HostStatus | null
}

export interface HostInput {
  display_name: string
  ssh_target: string
  registry?: string | null
  extra_env?: Record<string, string>
  data_dir?: string | null
}

export type HostErrorKind =
  'unreachable' | 'auth' | 'timeout' | 'remote_command'

export interface RemoteTool {
  path: string
  version: string
}

export interface ConnectionTest {
  ok: boolean
  kind: HostErrorKind | null
  message: string | null
  system: { os: string; release: string; arch: string } | null
  home: string | null
  data_dir: string | null
  data_dir_problem: string | null
  data_dir_exists: boolean
  node: RemoteTool | null
  pnpm: RemoteTool | null
}

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
  host_id?: string
}

export interface DshVersion {
  id: string
  version: string
  dir: string
  dedicated_home: string
  host_id?: string
  bin_ready: boolean
  path_linked: boolean
}

export interface DshInstance {
  id: string
  name: string
  version_id: string
  home_id: string
  host_id?: string
  env_overrides: Record<string, string>
  default_profile: string | null
  last_profile: string | null
  icon?: string | null
  port?: number | null
}

export interface InstanceLocation {
  host_id: string
  host_name: string
  ssh_target: string
  path: string
  command: string
}

export interface LauncherSettings {
  minimize_to_tray: boolean
  autostart: boolean
  last_instance_id: string | null
  theme: ThemeMode
  log_level: LogLevel
  terminal: string
  proxy_enabled: boolean
  proxy_url: string
  proxy_port: number
  no_proxy: string
  proxy_apply_dsh: boolean
}

export type ThemeMode = 'light' | 'dark' | 'system'

export type LogLevel = 'debug' | 'info' | 'warn' | 'error'

export type FindingLevel = 'warn' | 'error'

export interface DoctorFinding {
  level: FindingLevel
  code: string
  message: string
}

export interface DoctorReport {
  instance_id: string
  profile: string
  findings: DoctorFinding[]
}

export interface LauncherUpdateInfo {
  current: string
  channel: 'dev' | 'stable'
  up_to_date: boolean
  latest: string | null
  url: string | null
  published_at: string | null
  has_installer: boolean
}

export type InstanceState =
  'stopped' | 'starting' | 'running' | 'exited' | 'unreachable'

export interface InstanceStatus {
  id: string
  state: InstanceState
  url: string | null
  remote_url?: string | null
  profile: string | null
  exit_code: number | null
}

export type ToolSource = 'manual' | 'system' | 'fallback'

export interface ToolStatus {
  installed: boolean
  version: string | null
  path: string | null
  source: ToolSource | null
  manual_path: string | null
}

export interface SkippedCandidate {
  tool: 'node' | 'pnpm'
  path: string
  version?: string
  reason: string
  major?: number
}

export interface RuntimeStatus {
  host_id: string
  node: ToolStatus
  pnpm: ToolStatus
  managers: string[]
  node_below_recommended: boolean
  requirements: ToolchainRequirements
  system_env_shell: string | null
  skipped_candidates: SkippedCandidate[]
}

export interface ToolchainRequirements {
  min_pnpm_major: number
  recommended_node_major: number
  pnpm_install_command: string
  node_install_command: string
}

export type TaskState = 'running' | 'done' | 'error' | 'cancelled'

export interface TaskInfo {
  id: string
  kind: string
  label: string
  version: string
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
  source?: 'npm' | 'github' | null
}

export type PluginViewMode =
  'managed' | 'not-running' | 'adopted' | 'unreachable'

export interface BundleItem {
  name: string
  rowCount: number
  title?: string | null
  description?: string | null
  version?: string | null
  enabled: boolean
  installed: boolean
  removable: boolean
  optional: boolean
  availability?: string | null
  readOnlyReason?: string | null
  errorCode?: string | null
}

export interface InstanceScope {
  instanceId?: string | null
}

export interface PluginView {
  mode: PluginViewMode
  instanceId?: string | null
  reason?: string | null
  bundles: BundleItem[]
}

export interface ChangeOutcome {
  changed: boolean
  application: string
  stage: string
  target: string
  errorCode?: string | null
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

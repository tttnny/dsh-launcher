// Mock adapter (browser preview): a localStorage-backed reference
// implementation of the backend command contract. Every command the public
// api object can invoke MUST have a case in mockCall — ci/api-contract.test.mjs
// enforces it so the two adapters cannot drift silently.

import type {
  BundleItem,
  ChangeOutcome,
  ConnectionTest,
  DshHome,
  DshInstance,
  DshVersion,
  Host,
  HostInput,
  HostStatus,
  HostWithStatus,
  InstanceStatus,
  LauncherSettings,
  PluginView,
  RemoteVersion,
  RuntimeStatus,
  RemoveBundleInput,
  SetBundleEnabledInput,
  SshAlias,
  TaskInfo,
  TaskProgress,
  TaskLog,
  ToolSource,
  ToolStatus,
} from './types'
import { LOCAL_HOST_ID } from './types'

export type Listener<T> = (payload: T) => void

const MOCK_KEY = 'dsh-launcher.mock.v1'

interface MockDb {
  hosts: Host[]
  homes: DshHome[]
  versions: DshVersion[]
  instances: DshInstance[]
  settings: LauncherSettings
  running: Record<string, InstanceStatus>
}

/** The built-in host the browser preview always has. */
function localHost(): Host {
  return {
    id: LOCAL_HOST_ID,
    display_name: '本机',
    ssh_target: '',
    registry: null,
    extra_env: {},
    data_dir: null,
    builtin: true,
  }
}

/** The preview's remote machine, so host-aware pages have two of them to show. */
const REMOTE_HOST_ID = 'host-aliyun'

/** 本机's data directory as the preview spells it (sample paths are Windows-style). */
const LOCAL_DATA_DIR = 'C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher'

function remoteHost(): Host {
  return {
    id: REMOTE_HOST_ID,
    display_name: '阿里云服务器',
    ssh_target: 'zjq_aliyun',
    registry: null,
    extra_env: {},
    data_dir: '/home/demo/.local/share/dsh-launcher',
    builtin: false,
  }
}

/**
 * Host statuses, runtime only — the mock's stand-in for the backend's status
 * map, which is not persisted either (a machine that is off is still saved).
 * The remote sample starts out 不可达 so that state is visible in the preview;
 * 「测试连接」 then turns it 可达.
 */
const mockHostStatuses: Record<string, HostStatus> = {
  [REMOTE_HOST_ID]: {
    state: 'unreachable',
    message: 'ssh: connect to host zjq_aliyun port 22: Connection refused。确认 ssh zjq_aliyun 这台机器开着、地址没写错',
  },
}

function seedDb(): MockDb {
  return {
    hosts: [localHost(), remoteHost()],
    homes: [
      { id: 'h-default', name: '默认 DSH_HOME', path: 'C:\\Users\\Administrator\\.dsh', host_id: LOCAL_HOST_ID },
      { id: 'h-lab', name: '实验室环境', path: 'D:\\dsh-homes\\lab', host_id: LOCAL_HOST_ID },
      { id: 'h-remote', name: '服务器默认', path: '/home/demo/.dsh', host_id: REMOTE_HOST_ID },
    ],
    versions: [
      { id: 'v-rc6', version: '0.1.0-rc.6', dir: `${LOCAL_DATA_DIR}\\versions\\0.1.0-rc.6`, dedicated_home: `${LOCAL_DATA_DIR}\\homes\\0.1.0-rc.6`, bin_ready: true, host_id: LOCAL_HOST_ID },
      { id: 'v-rc5', version: '0.1.0-rc.5', dir: `${LOCAL_DATA_DIR}\\versions\\0.1.0-rc.5`, dedicated_home: `${LOCAL_DATA_DIR}\\homes\\0.1.0-rc.5`, bin_ready: true, host_id: LOCAL_HOST_ID },
      { id: 'v-remote', version: '0.1.0-rc.6', dir: '/home/demo/.local/share/dsh-launcher/versions/0.1.0-rc.6', dedicated_home: '/home/demo/.local/share/dsh-launcher/homes/0.1.0-rc.6', bin_ready: true, host_id: REMOTE_HOST_ID },
    ],
    instances: [
      {
        id: 'i-main',
        name: '主实例',
        version_id: 'v-rc6',
        home_id: 'h-default',
        host_id: LOCAL_HOST_ID,
        env_overrides: { DSH_TELEMETRY_DISABLED: '1' },
        default_profile: 'web',
        last_profile: 'web',
      },
      {
        id: 'i-exp',
        name: '实验实例',
        version_id: 'v-rc5',
        home_id: 'h-lab',
        host_id: LOCAL_HOST_ID,
        env_overrides: {},
        default_profile: null,
        last_profile: null,
      },
      {
        id: 'i-remote',
        name: '服务器实例',
        version_id: 'v-remote',
        home_id: 'h-remote',
        host_id: REMOTE_HOST_ID,
        env_overrides: {},
        default_profile: 'web',
        last_profile: 'web',
      },
    ],
    settings: {
      locale: 'zh-CN',
      minimize_to_tray: true,
      autostart: false,
      last_instance_id: 'i-main',
      theme: 'system',
      log_level: 'info',
      terminal: 'system',
      proxy_enabled: false,
      proxy_url: 'http://127.0.0.1',
      proxy_port: 7890,
      no_proxy: '127.0.0.1,localhost,::1',
      proxy_apply_dsh: false,
    },
    running: {},
  }
}

function loadDb(): MockDb {
  try {
    const raw = localStorage.getItem(MOCK_KEY)
    if (raw) {
      const db = JSON.parse(raw) as MockDb
      // Backfill fields added after the mock db was persisted.
      db.settings.log_level = db.settings.log_level ?? 'info'
      db.settings.proxy_enabled = db.settings.proxy_enabled ?? false
      db.settings.proxy_url = db.settings.proxy_url ?? 'http://127.0.0.1'
      db.settings.proxy_port = db.settings.proxy_port ?? 7890
      db.settings.no_proxy = db.settings.no_proxy ?? '127.0.0.1,localhost,::1'
      db.settings.proxy_apply_dsh = db.settings.proxy_apply_dsh ?? false
      db.settings.terminal = db.settings.terminal ?? 'system'
      // Backfill the dedicated DSH_HOME added with the version-delete change.
      for (const v of db.versions) {
        v.dedicated_home =
          v.dedicated_home ??
          `${LOCAL_DATA_DIR}\\homes\\${v.version}`
        // Bin readiness is what the machine answers; a version stored before
        // the field existed was installed by the launcher, so it is ready.
        v.bin_ready = v.bin_ready ?? true
      }
      // Backfill host ownership, which every record gained at once: whatever
      // was stored before the field existed can only belong to 本机.
      // A preview stored before remote hosts existed knows only 本机; it gains
      // the sample remote machine once, so host-aware pages have two to show.
      // A stored db that already carries a host list is left as it is: the
      // sample machine must not come back after the user deletes it.
      const storedHosts = Array.isArray(db.hosts)
      db.hosts = db.hosts ?? [localHost()]
      for (const h of db.homes) h.host_id = h.host_id ?? LOCAL_HOST_ID
      for (const v of db.versions) v.host_id = v.host_id ?? LOCAL_HOST_ID
      for (const i of db.instances) i.host_id = i.host_id ?? LOCAL_HOST_ID
      if (!storedHosts) db.hosts.push(remoteHost())
      return db
    }
  } catch {
    // fall through to seed
  }
  const db = seedDb()
  localStorage.setItem(MOCK_KEY, JSON.stringify(db))
  return db
}

function saveDb(db: MockDb) {
  localStorage.setItem(MOCK_KEY, JSON.stringify(db))
}

function uuid(): string {
  return 'xxxxxxxx-xxxx-4xxx'.replace(/x/g, () => ((Math.random() * 16) | 0).toString(16))
}

// Simple event emitter used by the mock to mimic Tauri events.
const statusListeners = new Set<Listener<InstanceStatus>>()
const taskProgressListeners = new Set<Listener<TaskProgress>>()
const taskLogListeners = new Set<Listener<TaskLog>>()

function emitStatus(s: InstanceStatus) {
  statusListeners.forEach((fn) => fn(s))
}
function emitTaskProgress(p: TaskProgress) {
  taskProgressListeners.forEach((fn) => fn(p))
}
function emitTaskLog(l: TaskLog) {
  taskLogListeners.forEach((fn) => fn(l))
}

// Mock task storage (runtime only, like the real backend).
const mockTasks = new Map<string, TaskInfo>()
// Mock profiles created at runtime per home id.
const mockProfiles: Record<string, string[]> = {}
// Mock native plugin views per `homeId:profile`, mirroring what a running
// instance answers to `/api/pluginManager/listBundles` (localized titles, an
// installation-only layer, an unmounted layer, and one broken layer).
const mockNativeStore: Record<string, PluginView> = {}
// The hand-picked executables of the browser preview, **per host** — as the
// backend keys them. There is no backend here, so a path chosen on the setup
// page is only remembered in this module.
const mockToolchainOverrides: Record<string, Partial<Record<'node' | 'pnpm', string>>> = {}

function mockOverridesOf(hostId: string): Partial<Record<'node' | 'pnpm', string>> {
  if (!mockToolchainOverrides[hostId]) mockToolchainOverrides[hostId] = {}
  return mockToolchainOverrides[hostId]
}

/** Where each preview machine's tools are, as its own probe would report them. */
const MOCK_TOOLS: Record<string, { node: { version: string; path: string }; pnpm: { version: string; path: string }; managers: string[] }> = {
  [LOCAL_HOST_ID]: {
    node: { version: 'v22.14.0', path: '/usr/local/bin/node' },
    pnpm: { version: '11.17.0', path: '/usr/local/bin/pnpm' },
    managers: ['brew'],
  },
  [REMOTE_HOST_ID]: {
    node: { version: 'v22.14.0', path: '/home/demo/.nvm/versions/node/v22.14.0/bin/node' },
    pnpm: { version: '11.17.0', path: '/home/demo/.local/share/pnpm/pnpm' },
    managers: ['nvm'],
  },
}

/**
 * The preview's runtime status for one host: both tools are assumed to be
 * present, a tool that was picked by hand reports that path instead (the same
 * "the pick wins" rule the guard applies), and the remote sample also carries
 * the candidates its probe passed over — the apt Node 18 and the pnpm 9 a real
 * Linux machine very often has.
 */
function mockRuntimeStatus(hostId: string = LOCAL_HOST_ID): RuntimeStatus {
  const tools = MOCK_TOOLS[hostId]
  const overrides = mockOverridesOf(hostId)
  const tool = (name: 'node' | 'pnpm', fallbackSource: ToolSource): ToolStatus => {
    const base = tools?.[name]
    const manual = overrides[name]
    if (manual) {
      return {
        installed: true,
        version: base?.version ?? null,
        path: manual,
        source: 'manual',
        manual_path: manual,
      }
    }
    if (!base) {
      // A host the preview has never probed: saying "not found" is the honest
      // answer, and it is what a real fresh host reports.
      return { installed: false, version: null, path: null, source: null, manual_path: null }
    }
    return {
      installed: true,
      version: base.version,
      path: base.path,
      source: fallbackSource,
      manual_path: null,
    }
  }
  const remote = hostId !== LOCAL_HOST_ID && !!tools
  return {
    host_id: hostId,
    node: tool('node', 'system'),
    pnpm: tool('pnpm', remote ? 'fallback' : 'system'),
    managers: tools?.managers ?? [],
    node_below_recommended: false,
    requirements: remote
      ? {
          min_pnpm_major: 11,
          recommended_node_major: 20,
          pnpm_install_command: `ssh zjq_aliyun '/bin/bash -ilc "curl -fsSL https://get.pnpm.io/install.sh | sh -"'`,
          node_install_command: `ssh zjq_aliyun '/bin/bash -ilc "nvm install --lts"'`,
        }
      : {
          min_pnpm_major: 11,
          recommended_node_major: 20,
          pnpm_install_command: 'npm install -g pnpm',
          node_install_command: 'brew install node',
        },
    system_env_shell: remote ? '/bin/bash' : '/bin/zsh',
    skipped_candidates: remote
      ? [
          {
            tool: 'node',
            path: '/usr/bin/node',
            version: 'v18.19.1',
            reason: 'below-minimum',
            major: 18,
          },
          {
            tool: 'pnpm',
            path: '/usr/bin/pnpm',
            version: '9.15.0',
            reason: 'below-minimum',
            major: 9,
          },
        ]
      : [],
  }
}

/**
 * A refusal, shaped exactly like the backend's `ChangeOutcome` (design §4d/§4b):
 * `errorCode` carries the **raw** native code and `diagnostic` carries the
 * **localized** reason the UI renders verbatim. `changed:false`, never an error.
 */
function mockRefusal(stage: string, target: string, code: string): ChangeOutcome {
  const reasons: Record<string, string> = {
    'management-required': '该条目由 DSH 自身管理，不能在这里改动',
    unaddressable: '该条目由 DSH 自身管理，不能在这里改动',
    'not-removable': '该 Bundle 随 DSH 发布，只能停用不能删除',
    'unknown-plugin': '实例里找不到这个插件',
  }
  return {
    changed: false,
    application: 'failed',
    stage,
    target,
    errorCode: code,
    diagnostic: reasons[code] ?? (code || '实例未说明原因'),
    warnings: [],
  }
}

/**
 * Which instance answers for `(homeId, profile)`: the one the page was opened
 * from when it is itself running that profile, otherwise the first registered
 * match — the same rule as the backend's `running_for_profile`.
 */
function resolveRunningInstance(
  db: MockDb,
  homeId: string,
  profile: string,
  prefer?: string | null,
): string | null {
  const own = db.instances.filter((i) => i.home_id === homeId)
  const live = (i: DshInstance) =>
    db.running[i.id]?.state === 'running' && db.running[i.id]?.profile === profile
  const preferred = prefer ? own.find((i) => i.id === prefer && live(i)) : undefined
  return (preferred ?? own.find(live))?.id ?? null
}

/** Seed a native view on first read, so the Plugins page is usable. */
/**
 * What the profile's own files say, as the backend's offline projection reports
 * it: which layers it declares and installs, and nothing only a running
 * instance could know — no live state, availability, row count or removal rule.
 */
function offlineProjection(bundles: BundleItem[]): BundleItem[] {
  return bundles.map((b) => ({
    ...b,
    // A file has no title, no description and no count: those come from the
    // running instance's own list.
    title: null,
    description: null,
    rowCount: 0,
    removable: false,
    optional: false,
    availability: null,
    readOnlyReason: null,
    errorCode: null,
  }))
}

/** The layers this profile declares: the mock's stand-in for its `package.json`. */
function mockBundlesFor(homeId: string, profile: string): BundleItem[] {
  const key = `${homeId}:${profile}`
  if (!mockNativeStore[key]) {
    mockNativeStore[key] = {
      mode: 'managed',
      instanceId: null,
      reason: null,
      bundles: [
        {
          name: '@deepseek-ai/dsh-base',
          rowCount: 9,
          title: 'DSH 基础',
          description: null,
          version: '0.2.1-alpha.2',
          enabled: true,
          installed: true,
          removable: false,
          optional: false,
          availability: 'installation',
          readOnlyReason: 'management-required',
          errorCode: null,
        },
        {
          name: '@deepseek-ai/dsh-web-app',
          rowCount: 3,
          title: 'DSH Web App',
          description: null,
          version: '0.2.1-alpha.2',
          enabled: true,
          installed: true,
          removable: false,
          optional: false,
          availability: 'installation',
          readOnlyReason: 'management-required',
          errorCode: null,
        },
        {
          name: '@canglongcl/ds-bot',
          rowCount: 1,
          title: 'ds-bot',
          description: null,
          version: '^0.9.4',
          enabled: true,
          installed: true,
          removable: true,
          optional: true,
          availability: 'profile',
          readOnlyReason: null,
          errorCode: null,
        },
        // Disabled layers the user could re-mount; most bundles ship this way.
        {
          name: '@deepseek-ai/dsh-llm-anthropic',
          rowCount: 2,
          title: 'Anthropic LLM',
          description: null,
          version: '0.2.1-alpha.2',
          enabled: false,
          installed: false,
          removable: false,
          optional: true,
          availability: 'installation',
          readOnlyReason: null,
          errorCode: null,
        },
        {
          name: 'ds-better-sidebar',
          rowCount: 1,
          title: '侧边栏增强',
          description: null,
          version: '^1.2.0',
          enabled: false,
          installed: true,
          removable: true,
          optional: true,
          availability: 'profile',
          readOnlyReason: null,
          errorCode: null,
        },
        // A bundle whose package files cannot be read.
        {
          name: '@example/dsh-missing',
          rowCount: 0,
          title: '缺失的层',
          description: null,
          version: null,
          enabled: false,
          installed: false,
          removable: true,
          optional: true,
          availability: 'missing',
          readOnlyReason: null,
          errorCode: 'unknown-plugin',
        },
      ],
    }
  }
  return mockNativeStore[key].bundles
}

function nativeViewFor(
  homeId: string,
  profile: string,
  instanceId: string | null,
): PluginView {
  const bundles = mockBundlesFor(homeId, profile)
  if (!instanceId) {
    // No instance answers, so the list is the profile's own files — the same
    // fallback the backend takes, labelled read-only by `mode`.
    return {
      mode: 'not-running',
      instanceId: null,
      reason:
        '实例未在运行：启动该实例后即可在这里管理它的补丁层。',
      bundles: offlineProjection(bundles),
    }
  }
  // The list is per profile; which instance answers is per call.
  return { mode: 'managed', instanceId, reason: null, bundles }
}

function mockNewId(prefix: string): string {
  return `${prefix}-${uuid()}`
}

/**
 * The host a host-aware list belongs to. Absent means 本机 — the backend's own
 * rule (`config::resolve_host`) — so a list that was not told which machine it
 * is for never merges two of them.
 */
function hostIdArg(args?: Record<string, unknown>): string {
  const raw = args?.hostId ?? args?.host_id
  const id = raw === undefined || raw === null ? '' : String(raw).trim()
  return id || LOCAL_HOST_ID
}

/** Every host with the status the preview holds for it. */
function hostViews(db: MockDb): HostWithStatus[] {
  return db.hosts.map((h) => ({ ...h, status: mockHostStatuses[h.id] ?? null }))
}

/** One host of the preview's db, by the id a call named (本机 when it named none). */
function hostOf(db: MockDb, hostId: string): Host {
  return db.hosts.find((h) => h.id === hostId) ?? db.hosts[0] ?? localHost()
}

/** The data directory the preview pretends a host has. */
function hostDirOf(db: MockDb, hostId: string): string {
  return hostOf(db, hostId).data_dir ?? LOCAL_DATA_DIR
}

/**
 * The separator the preview's sample paths use for one host: 本机's samples are
 * Windows-style, the remote machine's are POSIX — joining a path has to follow
 * whichever directory it starts from.
 */
function sepOf(dir: string): string {
  return dir.includes('\\') ? '\\' : '/'
}

/**
 * A host the preview marks 不可达 answers nothing: the backend's per-host reads
 * fail there too, and a page that showed a list anyway would be showing the
 * launcher's memory instead of the machine's answer.
 */
function refuseUnreachable(host: Host) {
  if ((mockHostStatuses[host.id]?.state ?? 'reachable') === 'unreachable') {
    throw new Error(
      `连不上：${mockHostStatuses[host.id]?.message ?? `ssh ${host.ssh_target} 没有回应`}`,
    )
  }
}

/**
 * The launcher only touches 本机's paths with its own filesystem calls; a remote
 * host's path is a path on that server. Mirrors `Host::local_paths_only`.
 */
function refuseRemotePathTouches(host: Host, what: string) {
  if (!host.builtin) {
    throw new Error(`「${host.display_name}」上的${what}不在本机，启动器不会在本机动它`)
  }
}

/**
 * Each machine's home directory as the preview learned it from 「测试连接」.
 * `~` in a path typed for another host expands against **that** machine's home
 * — the backend asks the machine itself (`remote::home_path_script`), and the
 * preview cannot ask, so it keeps what the connection test reported.
 */
const mockHostHomes: Record<string, string> = { [REMOTE_HOST_ID]: '/home/demo' }

/**
 * Settles a DSH_HOME path **on a remote host**, the way that machine would:
 * `~` against its own home, `..` (and doubled or trailing slashes) by text, and
 * a relative path refused. Mirrors `remote::home_path_script`, which is why the
 * preview refuses the same inputs the real scripts do.
 */
function settleRemotePath(host: Host, raw: string): string {
  const home = mockHostHomes[host.id] ?? '/home/demo'
  let requested: string
  if (raw === '~') requested = home
  else if (raw.startsWith('~/')) requested = `${home}/${raw.slice(2)}`
  else if (raw.startsWith('~')) throw new Error(`不支持 ~用户名 形式的路径：${raw}`)
  else requested = raw
  if (!requested.startsWith('/')) throw new Error(`请填写绝对路径（以 / 开头）：${raw}`)

  const parts: string[] = []
  for (const segment of requested.split('/')) {
    if (segment === '' || segment === '.') continue
    if (segment === '..') parts.pop()
    else parts.push(segment)
  }
  return `/${parts.join('/')}`
}

export async function mockCall<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const db = loadDb()
  function fail(msg: string): never {
    throw new Error(msg)
  }

  switch (cmd) {
    // --- Hosts ---------------------------------------------------------------
    case 'list_hosts':
      return hostViews(db) as T
    case 'create_host': {
      const input = (args?.input ?? {}) as HostInput
      const name = String(input.display_name ?? '').trim()
      const target = String(input.ssh_target ?? '').trim()
      if (!name) fail('主机的显示名不能为空')
      if (!target) fail('请填写 ssh 目标：~/.ssh/config 里的别名，或 user@host 这样的字符串')
      if (db.hosts.some((h) => h.ssh_target === target)) fail('已经有一台主机用这个 ssh 目标')
      const host: Host = {
        id: mockNewId('host'),
        display_name: name,
        ssh_target: target,
        registry: input.registry ?? null,
        extra_env: input.extra_env ?? {},
        data_dir: input.data_dir ?? null,
        builtin: false,
      }
      db.hosts.push(host)
      saveDb(db)
      return host as T
    }
    case 'update_host': {
      const id = String(args?.id)
      const input = (args?.input ?? {}) as HostInput
      const host = db.hosts.find((h) => h.id === id)
      if (!host) fail(`主机不存在: ${id}`)
      host.display_name = String(input.display_name ?? '').trim() || host.display_name
      // 本机 does not go over ssh; a form must not be able to make it one.
      if (!host.builtin) host.ssh_target = String(input.ssh_target ?? '').trim()
      host.registry = input.registry ?? null
      host.extra_env = input.extra_env ?? {}
      host.data_dir = input.data_dir ?? null
      saveDb(db)
      return host as T
    }
    case 'delete_host': {
      const id = String(args?.id)
      const host = db.hosts.find((h) => h.id === id)
      if (!host) fail(`主机不存在: ${id}`)
      if (host.builtin) fail('本机是内置的，不能删除')
      // Only the launcher's records go: the machine keeps everything it has and
      // the instances running there keep running (ADR-0006 §5).
      db.hosts = db.hosts.filter((h) => h.id !== id)
      db.homes = db.homes.filter((h) => h.host_id !== id)
      db.versions = db.versions.filter((v) => v.host_id !== id)
      db.instances = db.instances.filter((i) => i.host_id !== id)
      delete mockHostStatuses[id]
      saveDb(db)
      return undefined as T
    }
    case 'test_host_connection': {
      const id = String(args?.id)
      const host = db.hosts.find((h) => h.id === id)
      if (!host) fail(`主机不存在: ${id}`)
      if (host.builtin) fail('本机就在这台 Mac 上，不需要测试连接')
      // A machine that answers: the sample the real script's JSON mirrors.
      const test: ConnectionTest = {
        ok: true,
        kind: null,
        message: null,
        system: { os: 'Linux', release: '6.8.0-45-generic', arch: 'x86_64' },
        home: '/home/demo',
        data_dir: host.data_dir ?? '/home/demo/.local/share/dsh-launcher',
        data_dir_exists: true,
        node: { path: '/home/linuxbrew/.linuxbrew/bin/node', version: 'v26.0.0' },
        pnpm: { path: '/home/demo/.local/share/pnpm/pnpm', version: '11.1.2' },
      }
      mockHostStatuses[id] = { state: 'reachable', message: null }
      mockHostHomes[id] = test.home ?? mockHostHomes[id] ?? ''
      if (!host.data_dir) {
        host.data_dir = test.data_dir
        saveDb(db)
      }
      return test as T
    }
    case 'list_ssh_aliases':
      return [
        { alias: 'zjq_aliyun', host_name: '47.98.1.2', user: 'root', port: 2222 },
        { alias: 'lab-gpu', host_name: '10.0.0.7', user: 'deploy', port: null },
      ] as T

    // --- DSH_HOME ------------------------------------------------------------
    case 'list_homes': {
      const host = hostOf(db, hostIdArg(args))
      // A DSH_HOME is a record the launcher keeps, so a machine the preview
      // cannot reach still has its list here; the one thing only that machine
      // can answer — whether it has a `~/.dsh` of its own — is skipped rather
      // than guessed at.
      const answers = (mockHostStatuses[host.id]?.state ?? 'reachable') !== 'unreachable'
      // Its own `~/.dsh` joins the list the first time it is asked — the
      // question 本机's load-time sweep asks about this Mac (ADR-0006).
      if (!host.builtin && answers) {
        const dsh = `${mockHostHomes[host.id] ?? '/home/demo'}/.dsh`
        const known = db.homes.some(
          (h) => (h.host_id ?? LOCAL_HOST_ID) === host.id && h.path === dsh,
        )
        if (!known) {
          db.homes.push({
            id: `home-user-dsh-${host.id}`,
            name: '用户默认 (~/.dsh)',
            path: dsh,
            host_id: host.id,
          })
          saveDb(db)
        }
      }
      return db.homes.filter((h) => (h.host_id ?? LOCAL_HOST_ID) === host.id) as T
    }
    case 'default_dedicated_home_path': {
      const name = String(args?.name ?? 'instance')
      const safe = name.replace(/[^\w一-龥.-]+/g, '_')
      const hostDir = hostDirOf(db, hostIdArg(args))
      // The sample paths are Windows-style on 本机 and POSIX on the remote
      // machine; the joined path follows whichever the host's directory is.
      const sep = hostDir.includes('\\') ? '\\' : '/'
      return `${hostDir}${sep}homes${sep}${safe}` as T
    }
    case 'create_home': {
      const host = hostOf(db, hostIdArg(args))
      const name = String(args?.name ?? '').trim()
      const raw = String(args?.path ?? '').trim()
      if (!name || !raw) fail('名称与路径不能为空')
      // 本机's path is this Mac's; another machine's is settled **there**, and
      // only the answer is recorded — a path that machine will not use is not
      // saved at all.
      const path = host.builtin ? raw : settleRemotePath(host, raw)
      const existing = db.homes.find(
        (h) => (h.host_id ?? LOCAL_HOST_ID) === host.id && h.path === path,
      )
      if (existing) return existing as T
      const home: DshHome = { id: `h-${uuid()}`, name, path, host_id: host.id }
      db.homes.push(home)
      saveDb(db)
      return home as T
    }
    case 'remove_home': {
      const id = String(args?.id)
      if (db.instances.some((i) => i.home_id === id)) fail('该 DSH_HOME 仍被实例引用，无法删除')
      db.homes = db.homes.filter((h) => h.id !== id)
      saveDb(db)
      return undefined as T
    }
    case 'list_versions': {
      const host = hostOf(db, hostIdArg(args))
      // A machine the preview cannot reach answers with an error, not with a
      // list: 「装了哪些」 is a claim only that machine can make.
      refuseUnreachable(host)
      return db.versions
        .filter((v) => (v.host_id ?? LOCAL_HOST_ID) === host.id)
        .map((v) => ({ ...v, bin_ready: v.bin_ready ?? true })) as T
    }
    case 'fetch_available_versions': {
      const host = hostOf(db, hostIdArg(args))
      // Only a host with a registry of its own is asked over ssh; a host
      // without one is listed from the same registry this Mac uses, so the
      // list is the local one either way in the preview.
      if (host.registry) {
        refuseUnreachable(host)
        return [
          { version: '0.1.0-rc.6', released_at: '2026-08-01T12:00:00Z', source: 'npm' },
          { version: '0.0.9-mirror.2', released_at: '2026-05-20T08:00:00Z', source: 'npm' },
          { version: '0.1.2-alpha.1', released_at: '2026-10-01T08:00:00Z', source: 'github' },
        ] as T
      }
      return [
        { version: '0.1.2-alpha.1', released_at: '2026-10-01T08:00:00Z', source: 'github' },
        { version: '0.1.0-rc.6', released_at: '2026-08-01T12:00:00Z' },
        { version: '0.1.0-rc.5', released_at: '2026-07-15T09:30:00Z' },
        { version: '0.1.0-rc.4', released_at: '2026-07-01T10:00:00Z' },
        { version: '0.1.0-rc.3', released_at: '2026-06-15T08:00:00Z' },
      ] as T
    }
    case 'start_install_version_task': {
      const version = String(args?.version ?? '').trim()
      return mockCall('start_create_instance_task', {
        name: version,
        version,
        home_id: null,
        dedicated: true,
        host_id: hostIdArg(args),
      })
    }
    case 'start_create_instance_task': {
      const name = String(args?.name ?? '').trim()
      const version = String(args?.version ?? '').trim()
      const dedicated = Boolean(args?.dedicated)
      const homeIdArg = args?.home_id as string | null
      const host = hostOf(db, hostIdArg(args))
      refuseUnreachable(host)
      if (!name) fail('实例名称不能为空')
      if (!version) fail('版本号不能为空')
      if (db.instances.some((i) => i.name === name)) fail('同名实例已存在')
      if ([...mockTasks.values()].some((t) => t.state === 'running' && t.instance_name === name)) {
        fail('同名实例的下载任务已在进行中')
      }
      // The same version on the same host is one install; two hosts are two
      // directories and may install it side by side, exactly as the backend
      // allows.
      if (
        [...mockTasks.values()].some(
          (t) =>
            t.state === 'running' &&
            t.kind === 'install-version' &&
            t.version === version &&
            t.host_id === host.id,
        )
      ) {
        fail(`版本 ${version} 的下载任务已在进行中`)
      }

      const hostDir = hostDirOf(db, host.id)
      const sep = sepOf(hostDir)
      // Dedicated HOME is only materialized when the task finishes, mirroring
      // the real backend's placeholder semantics.
      let dedicatedPath: string | null = null
      if (dedicated) {
        const safe = name.replace(/[^\w一-龥.-]+/g, '_')
        dedicatedPath = `${hostDir}${sep}homes${sep}${safe}`
      }
      if (!dedicated && (!homeIdArg || !db.homes.some((h) => h.id === homeIdArg))) {
        fail('请选择 DSH_HOME')
      }

      const task: TaskInfo = {
        id: mockNewId('t'),
        // The launcher's only long task, and the only `kind` the UI branches
        // on: see `installingVersions` in the launcher store.
        kind: 'install-version',
        label: `下载 DSH ${version} 并创建实例「${name}」`,
        version,
        host_id: host.id,
        state: 'running',
        percent: 0,
        created_at: Date.now(),
        message: null,
        instance_id: null,
        instance_name: name,
        logs: [],
      }
      mockTasks.set(task.id, task)
      emitTaskProgress({ id: task.id, state: 'running', percent: 0, message: null, instance_id: null })

      // Simulate npm --loglevel=http download + install + instance creation.
      const fakeLogs = [
        `npm http fetch GET 200 https://registry.npmjs.org/@deepseek-ai%2fdsh 120ms`,
        `npm http fetch GET 200 https://registry.npmjs.org/@deepseek-ai/dsh/-/dsh-${version}.tgz 480ms`,
        `npm info ok`,
        `added 213 packages in 2s`,
      ]
      let step = 0
      const timer = setInterval(() => {
        const t = mockTasks.get(task.id)
        if (!t || t.state !== 'running') {
          clearInterval(timer)
          return
        }
        if (step < fakeLogs.length) {
          const line = fakeLogs[step]
          t.logs.push(line)
          t.percent = Math.min(95, t.percent + 20)
          emitTaskLog({ id: task.id, line })
          emitTaskProgress({ id: task.id, state: 'running', percent: t.percent, message: null, instance_id: null })
          step += 1
          return
        }
        clearInterval(timer)
        const cur = loadDb()
        // Resolve the HOME now (dedicated HOME is created at completion time).
        let resolvedHomeId = homeIdArg
        if (dedicated && dedicatedPath) {
          const same = (a: string, b: string) =>
            host.id === LOCAL_HOST_ID
              ? a.replace(/\\/g, '/').toLowerCase() === b.replace(/\\/g, '/').toLowerCase()
              : a === b
          const existing = cur.homes.find((h) => same(h.path, dedicatedPath!))
          if (existing) {
            resolvedHomeId = existing.id
          } else {
            const home: DshHome = { id: mockNewId('h'), name, path: dedicatedPath!, host_id: host.id }
            cur.homes.push(home)
            resolvedHomeId = home.id
          }
        }
        // Install version record if missing — on that host.
        let ver = cur.versions.find((v) => v.version === version && (v.host_id ?? LOCAL_HOST_ID) === host.id)
        if (!ver) {
          ver = {
            id: mockNewId('v'),
            version,
            dir: `${hostDir}${sepOf(hostDir)}versions${sepOf(hostDir)}${version}`,
            dedicated_home: dedicatedPath ?? `${hostDir}${sepOf(hostDir)}homes${sepOf(hostDir)}${version}`,
            bin_ready: true,
            host_id: host.id,
          }
          cur.versions.push(ver)
        }
        const inst: DshInstance = {
          id: mockNewId('i'),
          name,
          version_id: ver.id,
          home_id: resolvedHomeId!,
          host_id: host.id,
          env_overrides: {},
          default_profile: null,
          last_profile: null,
        }
        cur.instances.push(inst)
        saveDb(cur)
        const doneTask = mockTasks.get(task.id)
        if (doneTask) {
          doneTask.state = 'done'
          doneTask.percent = 100
          doneTask.instance_id = inst.id
        }
        emitTaskProgress({ id: task.id, state: 'done', percent: 100, message: null, instance_id: inst.id })
      }, 600)

      return task.id as T
    }
    case 'get_runtime_status': {
      // Browser preview: assume the tools are there so the UI is usable, per host.
      const hostId = String(args?.hostId ?? LOCAL_HOST_ID)
      return mockRuntimeStatus(hostId) as T
    }
    case 'set_toolchain_override': {
      const tool = args?.tool
      if (tool !== 'node' && tool !== 'pnpm') fail('工具名必须是 node 或 pnpm')
      const path = typeof args?.path === 'string' ? args.path.trim() : ''
      const hostId = String(args?.hostId ?? LOCAL_HOST_ID)
      mockOverridesOf(hostId)[tool] = path || undefined
      return mockRuntimeStatus(hostId) as T
    }
    case 'list_tasks': {
      return [...mockTasks.values()].sort((a, b) => b.created_at - a.created_at) as T
    }
    case 'remove_task': {
      const id = String(args?.id)
      const t = mockTasks.get(id)
      if (!t) fail('任务不存在')
      if (t.state === 'running') fail('任务仍在运行中，请先取消')
      mockTasks.delete(id)
      return undefined as T
    }
    case 'cancel_task': {
      const id = String(args?.id)
      const t = mockTasks.get(id)
      if (!t) fail('任务不存在')
      t.state = 'cancelled'
      t.message = '已取消'
      emitTaskProgress({ id, state: 'cancelled', percent: t.percent, message: '已取消', instance_id: null })
      return undefined as T
    }
    case 'remove_version': {
      const id = String(args?.id)
      const host = hostOf(db, hostIdArg(args))
      refuseUnreachable(host)
      if (db.instances.some((i) => i.version_id === id)) fail('该版本仍被实例引用，无法删除')
      const removed = db.versions.find((v) => v.id === id && (v.host_id ?? LOCAL_HOST_ID) === host.id)
      db.versions = db.versions.filter((v) => v.id !== id)
      // The version's dedicated DSH_HOME record goes with it, by the same path
      // rule the backend deletes the directory with — and only that host's
      // record: the same path on another machine is another HOME.
      if (removed) {
        const same = (a: string, b: string) =>
          host.id === LOCAL_HOST_ID
            ? a.replace(/\\/g, '/').toLowerCase() === b.replace(/\\/g, '/').toLowerCase()
            : a === b
        db.homes = db.homes.filter(
          (h) => !((h.host_id ?? LOCAL_HOST_ID) === host.id && same(h.path, removed.dedicated_home)),
        )
      }
      saveDb(db)
      return undefined as T
    }
    case 'list_instances': {
      // The board's 全部 view is an explicit request; silence still means 本机.
      if (args?.allHosts) return db.instances as T
      const hostId = hostIdArg(args)
      return db.instances.filter((i) => (i.host_id ?? LOCAL_HOST_ID) === hostId) as T
    }
    case 'create_instance': {
      const name = String(args?.name ?? '').trim()
      const versionId = String(args?.versionId ?? '')
      const homeId = String(args?.homeId ?? '')
      const hostId = hostIdArg(args)
      if (!name) fail('实例名称不能为空')
      if (db.instances.some((i) => i.name === name)) fail('同名实例已存在')
      const version = db.versions.find((v) => v.id === versionId)
      if (!version) fail('DSH 版本不存在')
      if ((version.host_id ?? LOCAL_HOST_ID) !== hostId) fail('DSH 版本不属于该主机')
      const home = db.homes.find((h) => h.id === homeId)
      if (!home) fail('DSH_HOME 不存在')
      if ((home.host_id ?? LOCAL_HOST_ID) !== hostId) fail('DSH_HOME 不属于该主机')
      const inst: DshInstance = {
        id: mockNewId('i'),
        name,
        version_id: versionId,
        home_id: homeId,
        host_id: hostId,
        env_overrides: {},
        default_profile: null,
        last_profile: null,
      }
      db.instances.push(inst)
      saveDb(db)
      return inst as T
    }
    case 'delete_instance': {
      const id = String(args?.id)
      const hostId = hostIdArg(args)
      const inst = db.instances.find((i) => i.id === id)
      if (!inst || (inst.host_id ?? LOCAL_HOST_ID) !== hostId) fail('实例不存在')
      db.instances = db.instances.filter((i) => i.id !== id)
      delete db.running[id]
      if (db.settings.last_instance_id === id) db.settings.last_instance_id = null
      saveDb(db)
      return undefined as T
    }
    case 'update_instance': {
      const input = args?.input as DshInstance
      const hostId = hostIdArg(args)
      const existing = db.instances.find((i) => i.id === input.id)
      if (!existing || (existing.host_id ?? LOCAL_HOST_ID) !== hostId) fail('实例不存在')
      if (db.instances.some((i) => i.name === input.name && i.id !== input.id)) fail('同名实例已存在')
      // The binding's host is the record's, not the form's.
      const updated = { ...input, host_id: existing.host_id }
      db.instances = db.instances.map((i) => (i.id === input.id ? updated : i))
      saveDb(db)
      return updated as T
    }
    case 'set_instance_port': {
      const id = String(args?.instance_id)
      const raw = args?.port
      const n = typeof raw === 'number' ? raw : NaN
      const port = Number.isInteger(n) && n >= 1 && n <= 65535 ? n : null
      const inst = db.instances.find((i) => i.id === id)
      if (!inst) fail('实例不存在')
      inst.port = port
      saveDb(db)
      return inst as T
    }
    case 'list_profiles': {
      const homeId = String(args?.home_id)
      // Profiles are data on the machine that owns the DSH_HOME: a host the
      // preview cannot reach has no list to show.
      refuseUnreachable(hostOf(db, db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID))
      const home = db.homes.find((h) => h.id === homeId)
      if (!home) fail('DSH_HOME 不存在')
      // Mock: combine the default set with previously created profiles.
      const base: string[] = home.path.endsWith('.dsh') ? ['web', 'demo', 'pack'] : ['web']
      const extras = mockProfiles[homeId] ?? []
      return [...base, ...extras] as T
    }
    case 'create_profile': {
      const homeId = String(args?.home_id)
      // Profiles are data on the machine that owns the DSH_HOME: a host the
      // preview cannot reach has no list to show.
      refuseUnreachable(hostOf(db, db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID))
      const name = String(args?.name ?? '').trim()
      if (!name) fail('Profile 名称不能为空')
      if (name === '__temp__' || name === 'node_modules') fail(`「${name}」为保留名称，不能使用`)
      if (!/^[A-Za-z0-9._-]+$/.test(name)) fail('Profile 名称只能包含字母、数字、-、_、.')
      const base: string[] = []
      const home = db.homes.find((h) => h.id === homeId)
      if (home && home.path.endsWith('.dsh')) base.push(...['web', 'demo', 'pack'])
      mockProfiles[homeId] = mockProfiles[homeId] ?? []
      if (base.includes(name) || mockProfiles[homeId].includes(name)) fail(`Profile「${name}」已存在`)
      mockProfiles[homeId].push(name)
      return name as T
    }
    case 'copy_profile': {
      const homeId = String(args?.home_id)
      // Profiles are data on the machine that owns the DSH_HOME: a host the
      // preview cannot reach has no list to show.
      refuseUnreachable(hostOf(db, db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID))
      const source = String(args?.source ?? '')
      const name = String(args?.name ?? '').trim()
      if (!name) fail('Profile 名称不能为空')
      if (name === '__temp__' || name === 'node_modules') fail(`「${name}」为保留名称，不能使用`)
      if (!/^[A-Za-z0-9._-]+$/.test(name)) fail('Profile 名称只能包含字母、数字、-、_、.')
      const home = db.homes.find((h) => h.id === homeId)
      const base: string[] = []
      if (home && home.path.endsWith('.dsh')) base.push(...['web', 'demo', 'pack'])
      mockProfiles[homeId] = mockProfiles[homeId] ?? []
      const exists = (n: string) => base.includes(n) || mockProfiles[homeId].includes(n)
      if (!exists(source)) fail(`Profile「${source}」不存在`)
      if (exists(name)) fail(`Profile「${name}」已存在`)
      mockProfiles[homeId].push(name)
      return name as T
    }
    case 'rename_profile': {
      const homeId = String(args?.home_id)
      // Profiles are data on the machine that owns the DSH_HOME: a host the
      // preview cannot reach has no list to show.
      refuseUnreachable(hostOf(db, db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID))
      const oldName = String(args?.old_name ?? '')
      const newName = String(args?.new_name ?? '').trim()
      if (!newName) fail('Profile 名称不能为空')
      if (newName === '__temp__' || newName === 'node_modules') fail(`「${newName}」为保留名称，不能使用`)
      if (!/^[A-Za-z0-9._-]+$/.test(newName)) fail('Profile 名称只能包含字母、数字、-、_、.')
      const home = db.homes.find((h) => h.id === homeId)
      const base: string[] = []
      if (home && home.path.endsWith('.dsh')) base.push(...['web', 'demo', 'pack'])
      mockProfiles[homeId] = mockProfiles[homeId] ?? []
      const exists = (n: string) => base.includes(n) || mockProfiles[homeId].includes(n)
      if (!exists(oldName)) fail(`Profile「${oldName}」不存在`)
      if (exists(newName)) fail(`Profile「${newName}」已存在`)
      const idx = mockProfiles[homeId].indexOf(oldName)
      if (idx >= 0) mockProfiles[homeId][idx] = newName
      // Keep instance references in sync.
      for (const inst of db.instances) {
        if (inst.home_id === homeId) {
          if (inst.default_profile === oldName) inst.default_profile = newName
          if (inst.last_profile === oldName) inst.last_profile = newName
        }
      }
      saveDb(db)
      return newName as T
    }
    case 'delete_profile': {
      const homeId = String(args?.home_id)
      // Profiles are data on the machine that owns the DSH_HOME: a host the
      // preview cannot reach has no list to show.
      refuseUnreachable(hostOf(db, db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID))
      const name = String(args?.name ?? '')
      if (name === '__temp__' || name === 'node_modules') fail(`「${name}」为保留名称，不能删除`)
      const home = db.homes.find((h) => h.id === homeId)
      const base: string[] = []
      if (home && home.path.endsWith('.dsh')) base.push(...['web', 'demo', 'pack'])
      mockProfiles[homeId] = mockProfiles[homeId] ?? []
      const exists = (n: string) => base.includes(n) || mockProfiles[homeId].includes(n)
      if (!exists(name)) fail(`Profile「${name}」不存在`)
      mockProfiles[homeId] = mockProfiles[homeId].filter((n) => n !== name)
      for (const inst of db.instances) {
        if (inst.home_id === homeId) {
          if (inst.default_profile === name) inst.default_profile = null
          if (inst.last_profile === name) inst.last_profile = null
        }
      }
      saveDb(db)
      return undefined as T
    }
    case 'start_instance': {
      const id = String(args?.id)
      const profile = String(args?.profile)
      if (db.running[id]?.state === 'running' || db.running[id]?.state === 'starting') fail('实例已在运行')
      const inst = db.instances.find((i) => i.id === id)
      if (!inst) fail('实例不存在')
      inst.last_profile = profile
      const starting: InstanceStatus = { id, state: 'starting', url: null, profile, exit_code: null }
      db.running[id] = starting
      saveDb(db)
      emitStatus(starting)
      const remote = !hostOf(db, inst.host_id ?? LOCAL_HOST_ID).builtin
      setTimeout(() => {
        const cur = loadDb()
        if (cur.running[id]?.state !== 'starting') return
        const running: InstanceStatus = {
          id,
          state: 'running',
          // 本机可达的地址：远端实例走启动器的端口转发，远端位置另说一处。
          // 样本故意让两个端口不同（远端 3080 在本机被占的情形），好让界面上
          // 「远端位置」与「本机地址」这两件事在浏览器预览里也看得出区别。
          url: remote
            ? 'http://127.0.0.1:41080/?token=demo-token-remote-instance'
            : `http://127.0.0.1:${30000 + Math.floor(Math.random() * 20000)}`,
          remote_url: remote ? 'http://127.0.0.1:3080/?token=demo-token-remote-instance' : null,
          profile,
          exit_code: null,
        }
        cur.running[id] = running
        saveDb(cur)
        emitStatus(running)
      }, 1500)
      return undefined as T
    }
    case 'stop_instance': {
      const id = String(args?.id)
      const stopped: InstanceStatus = { id, state: 'stopped', url: null, profile: null, exit_code: 0 }
      delete db.running[id]
      saveDb(db)
      emitStatus(stopped)
      return undefined as T
    }
    case 'restart_instance': {
      // Mirrors the backend: one transition that stops then starts on the
      // profile the instance was running.
      const id = String(args?.id)
      const entry = db.running[id]
      if (!entry) fail('实例未在运行')
      const stopped: InstanceStatus = { id, state: 'stopped', url: null, profile: null, exit_code: 0 }
      delete db.running[id]
      saveDb(db)
      emitStatus(stopped)
      return mockCall('start_instance', { id, profile: entry.profile })
    }
    case 'refresh_instance_status': {
      // The page's retry. The browser preview has no machine to ask, so it
      // mirrors what a reachable one answers: whatever the preview's own running
      // table says, or 「已停止」 when it holds nothing — the 「主机不可达」 the row
      // was showing comes off with it.
      const id = String(args?.id)
      const entry = db.running[id]
      if (entry) {
        emitStatus(entry)
      } else {
        emitStatus({ id, state: 'stopped', url: null, profile: null, exit_code: null })
      }
      return undefined as T
    }
    case 'open_instance_window': {
      // Browser preview mirrors the desktop behavior (system browser):
      // open the running instance URL in a new tab.
      const id = String(args?.id ?? '')
      const url = db.running[id]?.url
      // 与桌面端 open_instance_window 未就绪报错保持一致。
      if (!url) fail('实例未在运行或尚未就绪')
      window.open(url, '_blank', 'noopener,noreferrer')
      return undefined as T
    }
    case 'open_external':
      // Browser preview: a real new tab is the expected behavior here.
      window.open(String(args?.url ?? ''), '_blank', 'noopener,noreferrer')
      return undefined as T
    case 'open_launcher_directory':
    case 'open_launcher_log':
    case 'open_instance_log':
      // Browser preview has no file manager; the target path is reported as-is.
      return LOCAL_DATA_DIR as T
    case 'open_instance_directory': {
      const inst = db.instances.find((i) => i.id === String(args?.instanceId))
      const home = db.homes.find((h) => h.id === inst?.home_id)
      refuseRemotePathTouches(hostOf(db, home?.host_id ?? LOCAL_HOST_ID), 'DSH_HOME 目录')
      return LOCAL_DATA_DIR as T
    }
    case 'instance_remote_location': {
      const inst = db.instances.find((i) => i.id === String(args?.instanceId))
      const home = db.homes.find((h) => h.id === inst?.home_id)
      const host = hostOf(db, home?.host_id ?? LOCAL_HOST_ID)
      if (host.builtin) fail('本机实例的目录就在这台 Mac 上，用「打开目录」')
      const path = String(home?.path ?? '')
      return {
        host_id: host.id,
        host_name: host.display_name,
        ssh_target: host.ssh_target,
        path,
        command: `ssh ${host.ssh_target} '/bin/bash -ilc "ls -la ${path}"'`,
      } as T
    }
    case 'open_home_directory': {
      const home = db.homes.find((h) => h.id === String(args?.homeId))
      refuseRemotePathTouches(hostOf(db, home?.host_id ?? LOCAL_HOST_ID), 'DSH_HOME 目录')
      return LOCAL_DATA_DIR as T
    }
    case 'get_launcher_directory':
      return LOCAL_DATA_DIR as T
    case 'pending_deep_link':
      return null as T
    case 'set_instance_icon':
    case 'clear_instance_icon':
      return undefined as T
    case 'read_instance_icon':
      return null as T
    case 'open_instance_terminal':
      return `DSH ${String(args?.instanceId ?? args?.instance_id ?? '')}` as T
    case 'list_instance_status': {
      // Mirrors `process::list_statuses`: the machine that runs an instance
      // decides what may be said about it. A record with no process says
      // 「已停止」 — unless its machine is one the launcher cannot reach, which
      // is 「主机不可达」 and deliberately not a relabelling of 已停止: that
      // reading is what makes a user start a second DSH on a server (ADR-0007).
      const rows: InstanceStatus[] = Object.values(db.running)
      for (const inst of db.instances) {
        if (rows.some((r) => r.id === inst.id)) continue
        const host = hostOf(db, inst.host_id ?? LOCAL_HOST_ID)
        if ((mockHostStatuses[host.id]?.state ?? 'reachable') !== 'unreachable') continue
        rows.push({
          id: inst.id,
          state: 'unreachable',
          url: null,
          remote_url: null,
          profile: inst.last_profile ?? inst.default_profile ?? null,
          exit_code: null,
        })
      }
      return rows as T
    }
    case 'check_instance_health':
      // Browser preview has no real dependency tree: report healthy.
      return {
        instance_id: String(args?.instance_id ?? ''),
        profile: String(args?.profile ?? ''),
        findings: [],
      } as T
    case 'get_settings':
      return db.settings as T
    case 'update_settings': {
      db.settings = { ...db.settings, ...(args?.settings as Partial<LauncherSettings>) }
      saveDb(db)
      return db.settings as T
    }
    case 'check_launcher_update': {
      // Browser preview: honor the channel; the release channel reports the
      // same fake dev build as up-to-date so the filter is observable.
      const channel = (args?.channel as string | undefined) ?? 'dev'
      const dev = channel !== 'release'
      return {
        current: '0.2.0-dev.1',
        channel: dev ? 'dev' : 'stable',
        up_to_date: !dev,
        latest: dev ? '0.2.0-dev.2' : null,
        url: dev ? 'https://github.com/tttnny/dsh-launcher/releases' : null,
        published_at: dev ? new Date().toISOString() : null,
      } as T
    }
    case 'list_plugins': {
      const homeId = String(args?.home_id)
      const profile = String(args?.profile ?? 'web')
      // Mirror the backend: the view is managed only while an instance of this
      // HOME is running this profile, and that instance is the one the page was
      // opened from when it is still the candidate — otherwise the read-only
      // not-running stub.
      const instanceId = resolveRunningInstance(
        db,
        homeId,
        profile,
        args?.instance_id as string | null | undefined,
      )
      return nativeViewFor(homeId, profile, instanceId) as T
    }
    case 'set_bundle_enabled': {
      const input = args?.input as SetBundleEnabledInput
      const homeId = input.homeId
      const profile = input.profile
      const instanceId = resolveRunningInstance(db, homeId, profile, input.instanceId)
      // No instance → a plain error (mirrors the backend Err), not a refusal.
      if (!instanceId) fail('实例未在运行，请先启动实例')
      const view = nativeViewFor(homeId, profile, instanceId)
      const bundle = view.bundles.find((b) => b.name === input.name)
      const refusal = (code: string): ChangeOutcome =>
        mockRefusal('enable', input.name, code)
      if (!bundle) return refusal('unknown-plugin') as T
      // A missing package's files cannot be read, so the layer cannot mount.
      // `removable:false` does NOT block this toggle — it only forbids deletion
      // (the delete rule is a separate concern).
      if (bundle.availability === 'missing') return refusal('unknown-plugin') as T
      bundle.enabled = input.enabled
      return {
        changed: true,
        application: 'applied',
        stage: 'enable',
        target: input.name,
        errorCode: null,
        diagnostic: null,
        warnings: [],
      } as T
    }
    case 'remove_bundle': {
      const input = args?.input as RemoveBundleInput
      const homeId = input.homeId
      const profile = input.profile
      const instanceId = resolveRunningInstance(db, homeId, profile, input.instanceId)
      // No instance → a plain error (mirrors the backend Err), not a refusal.
      if (!instanceId) fail('实例未在运行，请先启动实例')
      const view = nativeViewFor(homeId, profile, instanceId)
      const index = view.bundles.findIndex((b) => b.name === input.name)
      if (index < 0) {
        // The instance answers a refusal (not an error) with the code only.
        return mockRefusal('remove', input.name, 'unknown-plugin') as T
      }
      // Bundles that ship with DSH can only be disabled, never removed — the
      // instance refuses with `not-removable` (HTTP 200, changed:false).
      if (!view.bundles[index].removable) {
        return mockRefusal('remove', input.name, 'not-removable') as T
      }
      view.bundles.splice(index, 1)
      return {
        changed: true,
        application: 'applied',
        stage: 'remove',
        target: input.name,
        errorCode: null,
        diagnostic: null,
        warnings: [],
      } as T
    }
    default:
      fail(`mock: unknown command ${cmd}`)
  }
}


// ---------------------------------------------------------------------------
// Mock event seam (mirrors the Tauri event bridge in tauri.ts)
// ---------------------------------------------------------------------------

export function subscribeMockInstanceStatus(cb: Listener<InstanceStatus>): () => void {
  statusListeners.add(cb)
  return () => statusListeners.delete(cb)
}

export function subscribeMockTaskProgress(cb: Listener<TaskProgress>): () => void {
  taskProgressListeners.add(cb)
  return () => taskProgressListeners.delete(cb)
}

export function subscribeMockTaskLog(cb: Listener<TaskLog>): () => void {
  taskLogListeners.add(cb)
  return () => taskLogListeners.delete(cb)
}

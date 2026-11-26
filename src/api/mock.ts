import type {
  BundleItem,
  ChangeOutcome,
  ConnectionTest,
  DshHome,
  DshInstance,
  DshVersion,
  FullDiskAccessStatus,
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
import { directoryCommand } from '../remoteCommand'

export type Listener<T> = (payload: T) => void

const MOCK_KEY = 'dsh-launcher.mock.v1'

interface MockDb {
  hosts: Host[]
  homes: DshHome[]
  versions: DshVersion[]
  instances: DshInstance[]
  settings: LauncherSettings
  running: Record<string, InstanceStatus>
  path_link: Record<string, string | null>
}

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

const REMOTE_HOST_ID = 'host-aliyun'

const LOCAL_DATA_DIR = '/Users/demo/Library/Application Support/dsh-launcher'

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

const mockHostStatuses: Record<string, HostStatus> = {
  [REMOTE_HOST_ID]: {
    state: 'unreachable',
    message:
      'ssh: connect to host zjq_aliyun port 22: Connection refused。确认 ssh zjq_aliyun 这台机器开着、地址没写错',
  },
}

function seedDb(): MockDb {
  return {
    hosts: [localHost(), remoteHost()],
    homes: [
      {
        id: 'h-default',
        name: '默认 DSH_HOME',
        path: '/Users/demo/.dsh',
        host_id: LOCAL_HOST_ID,
      },
      {
        id: 'h-lab',
        name: '实验室环境',
        path: '/Users/demo/dsh-homes/lab',
        host_id: LOCAL_HOST_ID,
      },
      {
        id: 'h-remote',
        name: '服务器默认',
        path: '/home/demo/.dsh',
        host_id: REMOTE_HOST_ID,
      },
    ],
    versions: [
      {
        id: 'v-rc6',
        version: '0.1.0-rc.6',
        dir: `${LOCAL_DATA_DIR}/versions/0.1.0-rc.6`,
        dedicated_home: `${LOCAL_DATA_DIR}/homes/0.1.0-rc.6`,
        bin_ready: true,
        path_linked: true,
        host_id: LOCAL_HOST_ID,
      },
      {
        id: 'v-rc5',
        version: '0.1.0-rc.5',
        dir: `${LOCAL_DATA_DIR}/versions/0.1.0-rc.5`,
        dedicated_home: `${LOCAL_DATA_DIR}/homes/0.1.0-rc.5`,
        bin_ready: true,
        path_linked: false,
        host_id: LOCAL_HOST_ID,
      },
      {
        id: 'v-remote',
        version: '0.1.0-rc.6',
        dir: '/home/demo/.local/share/dsh-launcher/versions/0.1.0-rc.6',
        dedicated_home: '/home/demo/.local/share/dsh-launcher/homes/0.1.0-rc.6',
        bin_ready: true,
        path_linked: false,
        host_id: REMOTE_HOST_ID,
      },
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
    path_link: { [LOCAL_HOST_ID]: 'v-rc6' },
  }
}

function loadDb(): MockDb {
  try {
    const raw = localStorage.getItem(MOCK_KEY)
    if (raw) {
      const db = JSON.parse(raw) as MockDb
      db.settings.log_level = db.settings.log_level ?? 'info'
      db.settings.proxy_enabled = db.settings.proxy_enabled ?? false
      db.settings.proxy_url = db.settings.proxy_url ?? 'http://127.0.0.1'
      db.settings.proxy_port = db.settings.proxy_port ?? 7890
      db.settings.no_proxy = db.settings.no_proxy ?? '127.0.0.1,localhost,::1'
      db.settings.proxy_apply_dsh = db.settings.proxy_apply_dsh ?? false
      db.settings.terminal = db.settings.terminal ?? 'system'
      for (const v of db.versions) {
        v.dedicated_home =
          v.dedicated_home ?? `${LOCAL_DATA_DIR}/homes/${v.version}`
        v.bin_ready = v.bin_ready ?? true
        v.path_linked = v.path_linked ?? false
      }
      if (typeof db.path_link === 'string' || db.path_link == null) {
        db.path_link = { [LOCAL_HOST_ID]: db.path_link ?? null }
      }
      const storedHosts = Array.isArray(db.hosts)
      db.hosts = db.hosts ?? [localHost()]
      for (const h of db.homes) h.host_id = h.host_id ?? LOCAL_HOST_ID
      for (const v of db.versions) v.host_id = v.host_id ?? LOCAL_HOST_ID
      for (const i of db.instances) i.host_id = i.host_id ?? LOCAL_HOST_ID
      if (!storedHosts) db.hosts.push(remoteHost())
      return db
    }
  } catch {}
  const db = seedDb()
  localStorage.setItem(MOCK_KEY, JSON.stringify(db))
  return db
}

function saveDb(db: MockDb) {
  localStorage.setItem(MOCK_KEY, JSON.stringify(db))
}

function uuid(): string {
  return 'xxxxxxxx-xxxx-4xxx'.replace(/x/g, () =>
    ((Math.random() * 16) | 0).toString(16),
  )
}

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

const mockTasks = new Map<string, TaskInfo>()
const mockProfiles: Record<string, string[]> = {}
const mockNativeStore: Record<string, PluginView> = {}
const mockToolchainOverrides: Record<
  string,
  Partial<Record<'node' | 'pnpm', string>>
> = {}

function mockOverridesOf(
  hostId: string,
): Partial<Record<'node' | 'pnpm', string>> {
  if (!mockToolchainOverrides[hostId]) mockToolchainOverrides[hostId] = {}
  return mockToolchainOverrides[hostId]
}

const MOCK_TOOLS: Record<
  string,
  {
    node: { version: string; path: string }
    pnpm: { version: string; path: string }
    managers: string[]
  }
> = {
  [LOCAL_HOST_ID]: {
    node: { version: 'v22.14.0', path: '/usr/local/bin/node' },
    pnpm: { version: '11.17.0', path: '/usr/local/bin/pnpm' },
    managers: ['brew'],
  },
  [REMOTE_HOST_ID]: {
    node: {
      version: 'v22.14.0',
      path: '/home/demo/.nvm/versions/node/v22.14.0/bin/node',
    },
    pnpm: { version: '11.17.0', path: '/home/demo/.local/share/pnpm/pnpm' },
    managers: ['nvm'],
  },
}

function mockRuntimeStatus(hostId: string = LOCAL_HOST_ID): RuntimeStatus {
  const tools = MOCK_TOOLS[hostId]
  const overrides = mockOverridesOf(hostId)
  const tool = (
    name: 'node' | 'pnpm',
    fallbackSource: ToolSource,
  ): ToolStatus => {
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
      return {
        installed: false,
        version: null,
        path: null,
        source: null,
        manual_path: null,
      }
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

function mockRefusal(
  stage: string,
  target: string,
  code: string,
): ChangeOutcome {
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

function resolveRunningInstance(
  db: MockDb,
  homeId: string,
  profile: string,
  prefer?: string | null,
): string | null {
  const own = db.instances.filter((i) => i.home_id === homeId)
  const live = (i: DshInstance) =>
    db.running[i.id]?.state === 'running' &&
    db.running[i.id]?.profile === profile
  const preferred = prefer
    ? own.find((i) => i.id === prefer && live(i))
    : undefined
  return (preferred ?? own.find(live))?.id ?? null
}

function offlineProjection(bundles: BundleItem[]): BundleItem[] {
  return bundles.map((b) => ({
    ...b,
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

function hostUnreachableReason(host: Host): string {
  const detail =
    mockHostStatuses[host.id]?.message?.trim() || '那台机器没有回应'
  return `主机「${host.display_name}」现在连不上（${detail}）。那台机器上的实例问不到，Profile 文件也读不到；恢复连接后点「重试」。`
}

function hostOutOfReach(db: MockDb, homeId: string): Host | null {
  const home = db.homes.find((h) => h.id === homeId)
  const host = home ? hostOf(db, home.host_id ?? LOCAL_HOST_ID) : undefined
  return host &&
    (mockHostStatuses[host.id]?.state ?? 'reachable') === 'unreachable'
    ? host
    : null
}

function nativeViewFor(
  db: MockDb,
  homeId: string,
  profile: string,
  instanceId: string | null,
): PluginView {
  const bundles = mockBundlesFor(homeId, profile)
  const unreachable = hostOutOfReach(db, homeId)
  if (unreachable) {
    return {
      mode: 'unreachable',
      instanceId: null,
      reason: hostUnreachableReason(unreachable),
      bundles: [],
    }
  }
  if (!instanceId) {
    return {
      mode: 'not-running',
      instanceId: null,
      reason: '实例未在运行：启动该实例后即可在这里管理它的补丁层。',
      bundles: offlineProjection(bundles),
    }
  }
  return { mode: 'managed', instanceId, reason: null, bundles }
}

const MOCK_FDA_KEY = 'dsh-launcher.mock.full-disk-access'

function mockFullDiskAccessStatus(): FullDiskAccessStatus {
  const state = localStorage.getItem(MOCK_FDA_KEY)
  return {
    supported: true,
    granted: state === 'granted',
    app_path: '/Applications/dsh-launcher.app',
    error: state === 'unknown' ? '浏览器预览：读不到受保护的目录' : null,
  }
}

function mockNewId(prefix: string): string {
  return `${prefix}-${uuid()}`
}

const SNAKE_CASE_COMMANDS = new Set([
  'start_install_version_task',
  'list_profiles',
  'create_profile',
  'copy_profile',
  'rename_profile',
  'delete_profile',
  'check_instance_health',
  'list_plugins',
  'set_bundle_enabled',
  'remove_bundle',
])

function backendKey(cmd: string, name: string): string {
  if (!SNAKE_CASE_COMMANDS.has(cmd)) return name
  return name.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`)
}

function argOf(
  args: Record<string, unknown> | undefined,
  cmd: string,
  name: string,
): unknown {
  return args?.[backendKey(cmd, name)]
}

function hostIdArg(cmd: string, args?: Record<string, unknown>): string {
  const raw = argOf(args, cmd, 'hostId')
  const id = raw === undefined || raw === null ? '' : String(raw).trim()
  return id || LOCAL_HOST_ID
}

function hostViews(db: MockDb): HostWithStatus[] {
  return db.hosts.map((h) => ({ ...h, status: mockHostStatuses[h.id] ?? null }))
}

function hostOf(db: MockDb, hostId: string): Host {
  return db.hosts.find((h) => h.id === hostId) ?? db.hosts[0] ?? localHost()
}

function hostDirOf(db: MockDb, hostId: string): string {
  return hostOf(db, hostId).data_dir ?? LOCAL_DATA_DIR
}

function sepOf(dir: string): string {
  return dir.includes('\\') ? '\\' : '/'
}

function settleRemoteDataDir(
  host: Host,
  typed: string | null | undefined,
): { dir: string | null; problem: string | null } {
  const value = (typed ?? '').trim()
  try {
    return {
      dir: settleRemotePath(host, value || '~/.local/share/dsh-launcher'),
      problem: null,
    }
  } catch (e) {
    return { dir: null, problem: e instanceof Error ? e.message : String(e) }
  }
}

function refuseUnreachable(host: Host) {
  if ((mockHostStatuses[host.id]?.state ?? 'reachable') === 'unreachable') {
    throw new Error(
      `连不上：${mockHostStatuses[host.id]?.message ?? `ssh ${host.ssh_target} 没有回应`}`,
    )
  }
}

function refuseRemotePathTouches(host: Host, what: string) {
  if (!host.builtin) {
    throw new Error(
      `「${host.display_name}」上的${what}不在本机，启动器不会在本机动它`,
    )
  }
}

const mockHostHomes: Record<string, string> = { [REMOTE_HOST_ID]: '/home/demo' }

function settleRemotePath(host: Host, raw: string): string {
  const home = mockHostHomes[host.id] ?? '/home/demo'
  let requested: string
  if (raw === '~') requested = home
  else if (raw.startsWith('~/')) requested = `${home}/${raw.slice(2)}`
  else if (raw.startsWith('~'))
    throw new Error(`不支持 ~用户名 形式的路径：${raw}`)
  else requested = raw
  if (!requested.startsWith('/'))
    throw new Error(`请填写绝对路径（以 / 开头）：${raw}`)

  const parts: string[] = []
  for (const segment of requested.split('/')) {
    if (segment === '' || segment === '.') continue
    if (segment === '..') parts.pop()
    else parts.push(segment)
  }
  return `/${parts.join('/')}`
}

export async function mockCall<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const db = loadDb()
  function fail(msg: string): never {
    throw new Error(msg)
  }

  switch (cmd) {
    case 'list_hosts':
      return hostViews(db) as T
    case 'create_host': {
      const input = (args?.input ?? {}) as HostInput
      const name = String(input.display_name ?? '').trim()
      const target = String(input.ssh_target ?? '').trim()
      if (!name) fail('主机的显示名不能为空')
      if (!target)
        fail(
          '请填写 ssh 目标：~/.ssh/config 里的别名，或 user@host 这样的字符串',
        )
      if (db.hosts.some((h) => h.ssh_target === target))
        fail('已经有一台主机用这个 ssh 目标')
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
      host.display_name =
        String(input.display_name ?? '').trim() || host.display_name
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
      const settled = settleRemoteDataDir(host, host.data_dir)
      const test: ConnectionTest = {
        ok: true,
        kind: null,
        message: null,
        system: { os: 'Linux', release: '6.8.0-45-generic', arch: 'x86_64' },
        home: mockHostHomes[id] ?? '/home/demo',
        data_dir: settled.dir,
        data_dir_problem: settled.problem,
        data_dir_exists: settled.dir !== null,
        node: {
          path: '/home/linuxbrew/.linuxbrew/bin/node',
          version: 'v26.0.0',
        },
        pnpm: { path: '/home/demo/.local/share/pnpm/pnpm', version: '11.1.2' },
      }
      mockHostStatuses[id] = { state: 'reachable', message: null }
      mockHostHomes[id] = test.home ?? mockHostHomes[id] ?? ''
      if (settled.dir && host.data_dir !== settled.dir) {
        host.data_dir = settled.dir
        saveDb(db)
      }
      return test as T
    }
    case 'list_ssh_aliases':
      return [
        {
          alias: 'zjq_aliyun',
          host_name: '47.98.1.2',
          user: 'root',
          port: 2222,
        },
        { alias: 'lab-gpu', host_name: '10.0.0.7', user: 'deploy', port: null },
      ] as T

    case 'list_homes': {
      const host = hostOf(db, hostIdArg(cmd, args))
      const answers =
        (mockHostStatuses[host.id]?.state ?? 'reachable') !== 'unreachable'
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
      return db.homes.filter(
        (h) => (h.host_id ?? LOCAL_HOST_ID) === host.id,
      ) as T
    }
    case 'default_dedicated_home_path': {
      const name = String(args?.name ?? 'instance')
      const safe = name.replace(/[^\w一-龥.-]+/g, '_')
      const hostDir = hostDirOf(db, hostIdArg(cmd, args))
      const sep = hostDir.includes('\\') ? '\\' : '/'
      return `${hostDir}${sep}homes${sep}${safe}` as T
    }
    case 'create_home': {
      const host = hostOf(db, hostIdArg(cmd, args))
      const name = String(args?.name ?? '').trim()
      const raw = String(args?.path ?? '').trim()
      if (!name || !raw) fail('名称与路径不能为空')
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
      const hostId = hostIdArg(cmd, args)
      if (db.instances.some((i) => i.home_id === id))
        fail('该 DSH_HOME 仍被实例引用，无法删除')
      const home = db.homes.find((h) => h.id === id)
      if (!home || (home.host_id ?? LOCAL_HOST_ID) !== hostId)
        fail('该主机下没有这个 DSH_HOME')
      db.homes = db.homes.filter((h) => h.id !== id)
      saveDb(db)
      return undefined as T
    }
    case 'list_versions': {
      const host = hostOf(db, hostIdArg(cmd, args))
      refuseUnreachable(host)
      return db.versions
        .filter((v) => (v.host_id ?? LOCAL_HOST_ID) === host.id)
        .map((v) => ({
          ...v,
          bin_ready: v.bin_ready ?? true,
          path_linked: (db.path_link[host.id] ?? null) === v.id,
        })) as T
    }
    case 'fetch_available_versions': {
      const host = hostOf(db, hostIdArg(cmd, args))
      if (host.registry) {
        refuseUnreachable(host)
        return [
          {
            version: '0.1.0-rc.6',
            released_at: '2026-08-01T12:00:00Z',
            source: 'npm',
          },
          {
            version: '0.0.9-mirror.2',
            released_at: '2026-05-20T08:00:00Z',
            source: 'npm',
          },
          {
            version: '0.1.2-alpha.1',
            released_at: '2026-10-01T08:00:00Z',
            source: 'github',
          },
        ] as T
      }
      return [
        {
          version: '0.1.2-alpha.1',
          released_at: '2026-10-01T08:00:00Z',
          source: 'github',
        },
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
        host_id: hostIdArg(cmd, args),
      })
    }
    case 'start_create_instance_task': {
      const name = String(args?.name ?? '').trim()
      const version = String(args?.version ?? '').trim()
      const dedicated = Boolean(args?.dedicated)
      const homeIdArg = args?.home_id as string | null
      const host = hostOf(
        db,
        String(args?.host_id ?? '').trim() || LOCAL_HOST_ID,
      )
      refuseUnreachable(host)
      if (!name) fail('实例名称不能为空')
      if (!version) fail('版本号不能为空')
      if (
        db.instances.some(
          (i) => i.name === name && (i.host_id ?? LOCAL_HOST_ID) === host.id,
        )
      ) {
        fail('同名实例已存在')
      }
      if (
        [...mockTasks.values()].some(
          (t) =>
            t.state === 'running' &&
            t.instance_name === name &&
            t.host_id === host.id,
        )
      ) {
        fail('同名实例的下载任务已在进行中')
      }
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
      let dedicatedPath: string | null = null
      if (dedicated) {
        const safe = name.replace(/[^\w一-龥.-]+/g, '_')
        dedicatedPath = `${hostDir}${sep}homes${sep}${safe}`
      }
      if (
        !dedicated &&
        (!homeIdArg || !db.homes.some((h) => h.id === homeIdArg))
      ) {
        fail('请选择 DSH_HOME')
      }

      const task: TaskInfo = {
        id: mockNewId('t'),
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
      emitTaskProgress({
        id: task.id,
        state: 'running',
        percent: 0,
        message: null,
        instance_id: null,
      })

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
          emitTaskProgress({
            id: task.id,
            state: 'running',
            percent: t.percent,
            message: null,
            instance_id: null,
          })
          step += 1
          return
        }
        clearInterval(timer)
        const cur = loadDb()
        let resolvedHomeId = homeIdArg
        if (dedicated && dedicatedPath) {
          const same = (a: string, b: string) =>
            host.id === LOCAL_HOST_ID
              ? a.replace(/\\/g, '/').toLowerCase() ===
                b.replace(/\\/g, '/').toLowerCase()
              : a === b
          const existing = cur.homes.find((h) => same(h.path, dedicatedPath!))
          if (existing) {
            resolvedHomeId = existing.id
          } else {
            const home: DshHome = {
              id: mockNewId('h'),
              name,
              path: dedicatedPath!,
              host_id: host.id,
            }
            cur.homes.push(home)
            resolvedHomeId = home.id
          }
        }
        let ver = cur.versions.find(
          (v) =>
            v.version === version && (v.host_id ?? LOCAL_HOST_ID) === host.id,
        )
        if (!ver) {
          ver = {
            id: mockNewId('v'),
            version,
            dir: `${hostDir}${sepOf(hostDir)}versions${sepOf(hostDir)}${version}`,
            dedicated_home:
              dedicatedPath ??
              `${hostDir}${sepOf(hostDir)}homes${sepOf(hostDir)}${version}`,
            bin_ready: true,
            path_linked: false,
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
        emitTaskProgress({
          id: task.id,
          state: 'done',
          percent: 100,
          message: null,
          instance_id: inst.id,
        })
      }, 600)

      return task.id as T
    }
    case 'get_runtime_status': {
      return mockRuntimeStatus(hostIdArg(cmd, args)) as T
    }
    case 'set_toolchain_override': {
      const tool = args?.tool
      if (tool !== 'node' && tool !== 'pnpm') fail('工具名必须是 node 或 pnpm')
      const path = typeof args?.path === 'string' ? args.path.trim() : ''
      const hostId = hostIdArg(cmd, args)
      mockOverridesOf(hostId)[tool] = path || undefined
      return mockRuntimeStatus(hostId) as T
    }
    case 'list_tasks': {
      return [...mockTasks.values()].sort(
        (a, b) => b.created_at - a.created_at,
      ) as T
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
      emitTaskProgress({
        id,
        state: 'cancelled',
        percent: t.percent,
        message: '已取消',
        instance_id: null,
      })
      return undefined as T
    }
    case 'remove_version': {
      const id = String(args?.id)
      const host = hostOf(db, hostIdArg(cmd, args))
      refuseUnreachable(host)
      if (db.instances.some((i) => i.version_id === id))
        fail('该版本仍被实例引用，无法删除')
      const removed = db.versions.find(
        (v) => v.id === id && (v.host_id ?? LOCAL_HOST_ID) === host.id,
      )
      db.versions = db.versions.filter((v) => v.id !== id)
      if (removed) {
        const same = (a: string, b: string) =>
          host.id === LOCAL_HOST_ID
            ? a.replace(/\\/g, '/').toLowerCase() ===
              b.replace(/\\/g, '/').toLowerCase()
            : a === b
        db.homes = db.homes.filter(
          (h) =>
            !(
              (h.host_id ?? LOCAL_HOST_ID) === host.id &&
              same(h.path, removed.dedicated_home)
            ),
        )
      }
      if (db.path_link[host.id] === id) db.path_link[host.id] = null
      saveDb(db)
      return undefined as T
    }
    case 'set_version_path_link': {
      const host = hostOf(db, hostIdArg(cmd, args))
      refuseUnreachable(host)
      const id = args?.id == null ? null : String(args.id)
      if (id === null) {
        db.path_link[host.id] = null
        saveDb(db)
        return undefined as T
      }
      const version = db.versions.find(
        (v) => v.id === id && (v.host_id ?? LOCAL_HOST_ID) === host.id,
      )
      if (!version) fail('版本不存在')
      db.path_link[host.id] = id
      saveDb(db)
      return undefined as T
    }
    case 'list_instances': {
      if (args?.allHosts) return db.instances as T
      const hostId = hostIdArg(cmd, args)
      return db.instances.filter(
        (i) => (i.host_id ?? LOCAL_HOST_ID) === hostId,
      ) as T
    }
    case 'create_instance': {
      const name = String(args?.name ?? '').trim()
      const versionId = String(args?.versionId ?? '')
      const homeId = String(args?.homeId ?? '')
      const hostId = hostIdArg(cmd, args)
      if (!name) fail('实例名称不能为空')
      if (
        db.instances.some(
          (i) => i.name === name && (i.host_id ?? LOCAL_HOST_ID) === hostId,
        )
      ) {
        fail('同名实例已存在')
      }
      const version = db.versions.find((v) => v.id === versionId)
      if (!version) fail('DSH 版本不存在')
      if ((version.host_id ?? LOCAL_HOST_ID) !== hostId)
        fail('DSH 版本不属于该主机')
      const home = db.homes.find((h) => h.id === homeId)
      if (!home) fail('DSH_HOME 不存在')
      if ((home.host_id ?? LOCAL_HOST_ID) !== hostId)
        fail('DSH_HOME 不属于该主机')
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
      const hostId = hostIdArg(cmd, args)
      const inst = db.instances.find((i) => i.id === id)
      if (!inst || (inst.host_id ?? LOCAL_HOST_ID) !== hostId)
        fail('实例不存在')
      db.instances = db.instances.filter((i) => i.id !== id)
      delete db.running[id]
      if (db.settings.last_instance_id === id)
        db.settings.last_instance_id = null
      saveDb(db)
      return undefined as T
    }
    case 'update_instance': {
      const input = args?.input as DshInstance
      const hostId = hostIdArg(cmd, args)
      const existing = db.instances.find((i) => i.id === input.id)
      if (!existing || (existing.host_id ?? LOCAL_HOST_ID) !== hostId)
        fail('实例不存在')
      if (
        db.instances.some(
          (i) =>
            i.name === input.name &&
            i.id !== input.id &&
            (i.host_id ?? LOCAL_HOST_ID) ===
              (existing.host_id ?? LOCAL_HOST_ID),
        )
      ) {
        fail('同名实例已存在')
      }
      const port =
        typeof input.port === 'number' &&
        Number.isInteger(input.port) &&
        input.port >= 1 &&
        input.port <= 65535
          ? input.port
          : null
      const updated = { ...input, port, host_id: existing.host_id }
      db.instances = db.instances.map((i) => (i.id === input.id ? updated : i))
      saveDb(db)
      return updated as T
    }
    case 'list_profiles': {
      const homeId = String(args?.home_id)
      refuseUnreachable(
        hostOf(
          db,
          db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID,
        ),
      )
      const home = db.homes.find((h) => h.id === homeId)
      if (!home) fail('DSH_HOME 不存在')
      const base: string[] = home.path.endsWith('.dsh')
        ? ['web', 'demo', 'pack']
        : ['web']
      const extras = mockProfiles[homeId] ?? []
      return [...base, ...extras] as T
    }
    case 'create_profile': {
      const homeId = String(args?.home_id)
      refuseUnreachable(
        hostOf(
          db,
          db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID,
        ),
      )
      const name = String(args?.name ?? '').trim()
      if (!name) fail('Profile 名称不能为空')
      if (name === '__temp__' || name === 'node_modules')
        fail(`「${name}」为保留名称，不能使用`)
      if (!/^[A-Za-z0-9._-]+$/.test(name))
        fail('Profile 名称只能包含字母、数字、-、_、.')
      const base: string[] = []
      const home = db.homes.find((h) => h.id === homeId)
      if (home && home.path.endsWith('.dsh'))
        base.push(...['web', 'demo', 'pack'])
      mockProfiles[homeId] = mockProfiles[homeId] ?? []
      if (base.includes(name) || mockProfiles[homeId].includes(name))
        fail(`Profile「${name}」已存在`)
      mockProfiles[homeId].push(name)
      return name as T
    }
    case 'copy_profile': {
      const homeId = String(args?.home_id)
      refuseUnreachable(
        hostOf(
          db,
          db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID,
        ),
      )
      const source = String(args?.source ?? '')
      const name = String(args?.name ?? '').trim()
      if (!name) fail('Profile 名称不能为空')
      if (name === '__temp__' || name === 'node_modules')
        fail(`「${name}」为保留名称，不能使用`)
      if (!/^[A-Za-z0-9._-]+$/.test(name))
        fail('Profile 名称只能包含字母、数字、-、_、.')
      const home = db.homes.find((h) => h.id === homeId)
      const base: string[] = []
      if (home && home.path.endsWith('.dsh'))
        base.push(...['web', 'demo', 'pack'])
      mockProfiles[homeId] = mockProfiles[homeId] ?? []
      const exists = (n: string) =>
        base.includes(n) || mockProfiles[homeId].includes(n)
      if (!exists(source)) fail(`Profile「${source}」不存在`)
      if (exists(name)) fail(`Profile「${name}」已存在`)
      mockProfiles[homeId].push(name)
      return name as T
    }
    case 'rename_profile': {
      const homeId = String(args?.home_id)
      refuseUnreachable(
        hostOf(
          db,
          db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID,
        ),
      )
      const oldName = String(args?.old_name ?? '')
      const newName = String(args?.new_name ?? '').trim()
      if (!newName) fail('Profile 名称不能为空')
      if (newName === '__temp__' || newName === 'node_modules')
        fail(`「${newName}」为保留名称，不能使用`)
      if (!/^[A-Za-z0-9._-]+$/.test(newName))
        fail('Profile 名称只能包含字母、数字、-、_、.')
      const home = db.homes.find((h) => h.id === homeId)
      const base: string[] = []
      if (home && home.path.endsWith('.dsh'))
        base.push(...['web', 'demo', 'pack'])
      mockProfiles[homeId] = mockProfiles[homeId] ?? []
      const exists = (n: string) =>
        base.includes(n) || mockProfiles[homeId].includes(n)
      if (!exists(oldName)) fail(`Profile「${oldName}」不存在`)
      if (exists(newName)) fail(`Profile「${newName}」已存在`)
      const idx = mockProfiles[homeId].indexOf(oldName)
      if (idx >= 0) mockProfiles[homeId][idx] = newName
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
      refuseUnreachable(
        hostOf(
          db,
          db.homes.find((h) => h.id === homeId)?.host_id ?? LOCAL_HOST_ID,
        ),
      )
      const name = String(args?.name ?? '')
      if (name === '__temp__' || name === 'node_modules')
        fail(`「${name}」为保留名称，不能删除`)
      const home = db.homes.find((h) => h.id === homeId)
      const base: string[] = []
      if (home && home.path.endsWith('.dsh'))
        base.push(...['web', 'demo', 'pack'])
      mockProfiles[homeId] = mockProfiles[homeId] ?? []
      const exists = (n: string) =>
        base.includes(n) || mockProfiles[homeId].includes(n)
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
      if (
        db.running[id]?.state === 'running' ||
        db.running[id]?.state === 'starting'
      )
        fail('实例已在运行')
      const inst = db.instances.find((i) => i.id === id)
      if (!inst) fail('实例不存在')
      inst.last_profile = profile
      const starting: InstanceStatus = {
        id,
        state: 'starting',
        url: null,
        profile,
        exit_code: null,
      }
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
          url: remote
            ? 'http://127.0.0.1:41080/?token=demo-token-remote-instance'
            : `http://127.0.0.1:${30000 + Math.floor(Math.random() * 20000)}`,
          remote_url: remote
            ? 'http://127.0.0.1:3080/?token=demo-token-remote-instance'
            : null,
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
      const stopped: InstanceStatus = {
        id,
        state: 'stopped',
        url: null,
        profile: null,
        exit_code: 0,
      }
      delete db.running[id]
      saveDb(db)
      emitStatus(stopped)
      return undefined as T
    }
    case 'restart_instance': {
      const id = String(args?.id)
      const entry = db.running[id]
      if (!entry) fail('实例未在运行')
      const stopped: InstanceStatus = {
        id,
        state: 'stopped',
        url: null,
        profile: null,
        exit_code: 0,
      }
      delete db.running[id]
      saveDb(db)
      emitStatus(stopped)
      return mockCall('start_instance', { id, profile: entry.profile })
    }
    case 'refresh_instance_status': {
      const id = String(args?.id)
      const inst = db.instances.find((i) => i.id === id)
      const host = hostOf(db, inst?.host_id ?? LOCAL_HOST_ID)
      if ((mockHostStatuses[host.id]?.state ?? 'reachable') === 'unreachable') {
        emitStatus({
          id,
          state: 'unreachable',
          url: null,
          remote_url: null,
          profile: inst?.last_profile ?? inst?.default_profile ?? null,
          exit_code: null,
        })
        return undefined as T
      }
      const entry = db.running[id]
      if (entry) {
        emitStatus(entry)
      } else {
        emitStatus({
          id,
          state: 'stopped',
          url: null,
          profile: null,
          exit_code: null,
        })
      }
      return undefined as T
    }
    case 'open_instance_window': {
      const id = String(args?.id ?? '')
      const url = db.running[id]?.url
      if (!url) fail('实例未在运行或尚未就绪')
      window.open(url, '_blank', 'noopener,noreferrer')
      return undefined as T
    }
    case 'open_external':
      window.open(String(args?.url ?? ''), '_blank', 'noopener,noreferrer')
      return undefined as T
    case 'open_launcher_directory':
    case 'open_launcher_log':
    case 'open_instance_log':
      return LOCAL_DATA_DIR as T
    case 'open_instance_directory': {
      const inst = db.instances.find((i) => i.id === String(args?.instanceId))
      const home = db.homes.find((h) => h.id === inst?.home_id)
      refuseRemotePathTouches(
        hostOf(db, home?.host_id ?? LOCAL_HOST_ID),
        'DSH_HOME 目录',
      )
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
        command: directoryCommand(host.ssh_target, path),
      } as T
    }
    case 'host_directory_command': {
      const host = hostOf(db, hostIdArg(cmd, args))
      if (host.builtin) fail('本机没有可复制的远程命令')
      const target = host.ssh_target.trim()
      if (!target) fail(`「${host.display_name}」还没有 ssh 目标`)
      const path = String(args?.path ?? '')
      return directoryCommand(target, path) as T
    }
    case 'open_home_directory': {
      const home = db.homes.find((h) => h.id === String(args?.homeId))
      refuseRemotePathTouches(
        hostOf(db, home?.host_id ?? LOCAL_HOST_ID),
        'DSH_HOME 目录',
      )
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
      return `DSH ${String(argOf(args, cmd, 'instanceId') ?? '')}` as T
    case 'list_instance_status': {
      const rows: InstanceStatus[] = Object.values(db.running)
      for (const inst of db.instances) {
        if (rows.some((r) => r.id === inst.id)) continue
        const host = hostOf(db, inst.host_id ?? LOCAL_HOST_ID)
        if ((mockHostStatuses[host.id]?.state ?? 'reachable') !== 'unreachable')
          continue
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
      return {
        instance_id: String(args?.instance_id ?? ''),
        profile: String(args?.profile ?? ''),
        findings: [],
      } as T
    case 'full_disk_access_status':
      return mockFullDiskAccessStatus() as T
    case 'open_full_disk_access_settings':
    case 'show_full_disk_access_drag_panel':
    case 'close_full_disk_access_drag_panel':
    case 'reveal_app_bundle':
      return undefined as T
    case 'get_settings':
      return db.settings as T
    case 'update_settings': {
      db.settings = {
        ...db.settings,
        ...(args?.settings as Partial<LauncherSettings>),
      }
      saveDb(db)
      return db.settings as T
    }
    case 'restart_launcher_update':
      return undefined as T
    case 'check_launcher_update': {
      return {
        current: '0.0.1-rc.1',
        channel: 'dev',
        up_to_date: false,
        latest: '0.0.1-rc.2',
        url: 'https://github.com/tttnny/dsh-launcher/releases/tag/v0.0.1-rc.2',
        published_at: new Date().toISOString(),
        has_installer: true,
        ready_to_restart: false,
      } as T
    }
    case 'list_plugins': {
      const homeId = String(args?.home_id)
      const profile = String(args?.profile ?? 'web')
      const instanceId = resolveRunningInstance(
        db,
        homeId,
        profile,
        args?.instance_id as string | null | undefined,
      )
      return nativeViewFor(db, homeId, profile, instanceId) as T
    }
    case 'set_bundle_enabled': {
      const input = args?.input as SetBundleEnabledInput
      const homeId = input.homeId
      const profile = input.profile
      const unreachable = hostOutOfReach(db, homeId)
      if (unreachable) fail(hostUnreachableReason(unreachable))
      const instanceId = resolveRunningInstance(
        db,
        homeId,
        profile,
        input.instanceId,
      )
      if (!instanceId) fail('实例未在运行，请先启动实例')
      const view = nativeViewFor(db, homeId, profile, instanceId)
      const bundle = view.bundles.find((b) => b.name === input.name)
      const refusal = (code: string): ChangeOutcome =>
        mockRefusal('enable', input.name, code)
      if (!bundle) return refusal('unknown-plugin') as T
      if (bundle.availability === 'missing')
        return refusal('unknown-plugin') as T
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
      const unreachable = hostOutOfReach(db, homeId)
      if (unreachable) fail(hostUnreachableReason(unreachable))
      const instanceId = resolveRunningInstance(
        db,
        homeId,
        profile,
        input.instanceId,
      )
      if (!instanceId) fail('实例未在运行，请先启动实例')
      const view = nativeViewFor(db, homeId, profile, instanceId)
      const index = view.bundles.findIndex((b) => b.name === input.name)
      if (index < 0) {
        return mockRefusal('remove', input.name, 'unknown-plugin') as T
      }
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

export function subscribeMockInstanceStatus(
  cb: Listener<InstanceStatus>,
): () => void {
  statusListeners.add(cb)
  return () => statusListeners.delete(cb)
}

export function subscribeMockTaskProgress(
  cb: Listener<TaskProgress>,
): () => void {
  taskProgressListeners.add(cb)
  return () => taskProgressListeners.delete(cb)
}

export function subscribeMockTaskLog(cb: Listener<TaskLog>): () => void {
  taskLogListeners.add(cb)
  return () => taskLogListeners.delete(cb)
}

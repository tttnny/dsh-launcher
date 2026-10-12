// Mock adapter (browser preview): a localStorage-backed reference
// implementation of the backend command contract. Every command the public
// api object can invoke MUST have a case in mockCall — ci/api-contract.test.mjs
// enforces it so the two adapters cannot drift silently.

import type {
  ChangeOutcome,
  DshHome,
  DshInstance,
  DshVersion,
  InstalledPlugin,
  InstanceStatus,
  LauncherSettings,
  PluginItem,
  PluginView,
  RemoteVersion,
  RuntimeStatus,
  SetBundleEnabledInput,
  SetPluginsEnabledInput,
  TaskInfo,
  TaskProgress,
  TaskLog,
  UninstallPluginInput,
} from './types'

export type Listener<T> = (payload: T) => void

const MOCK_KEY = 'dsh-launcher.mock.v1'

interface MockDb {
  homes: DshHome[]
  versions: DshVersion[]
  instances: DshInstance[]
  settings: LauncherSettings
  running: Record<string, InstanceStatus>
}

function seedDb(): MockDb {
  return {
    homes: [
      { id: 'h-default', name: '默认 DSH_HOME', path: 'C:\\Users\\Administrator\\.dsh' },
      { id: 'h-lab', name: '实验室环境', path: 'D:\\dsh-homes\\lab' },
    ],
    versions: [
      { id: 'v-rc6', version: '0.1.0-rc.6', dir: 'C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\versions\\0.1.0-rc.6', dedicated_home: 'C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\homes\\0.1.0-rc.6' },
      { id: 'v-rc5', version: '0.1.0-rc.5', dir: 'C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\versions\\0.1.0-rc.5', dedicated_home: 'C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\homes\\0.1.0-rc.5' },
    ],
    instances: [
      {
        id: 'i-main',
        name: '主实例',
        version_id: 'v-rc6',
        home_id: 'h-default',
        env_overrides: { DSH_TELEMETRY_DISABLED: '1' },
        default_profile: 'web',
        last_profile: 'web',
      },
      {
        id: 'i-exp',
        name: '实验实例',
        version_id: 'v-rc5',
        home_id: 'h-lab',
        env_overrides: {},
        default_profile: null,
        last_profile: null,
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
          `C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\homes\\${v.version}`
      }
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
// Mock plugin lists per `homeId:profile` (mirrors the backend's HOME+profile scope).
const mockPluginStore: Record<string, InstalledPlugin[]> = {}
// Mock native plugin views per `homeId:profile` (mirrors the instance's own
// /api/pluginManager lists; includes the @deepseek-ai/* core entries the old
// file reader filtered out, so the two are visibly different).
const mockNativeStore: Record<string, PluginView> = {}

/** One native entry with only the fields the mock needs; the rest default. */
function nativePlugin(
  entryId: string,
  moduleName: string,
  title: string,
  enabled: boolean,
  patchId: string | null,
  readOnlyReason: string | null,
): PluginItem {
  return {
    entryId,
    moduleName,
    title,
    description: null,
    enabled,
    fiberPhase: enabled ? 'active' : 'pending',
    patchId,
    readOnlyReason,
  }
}

/** Seed a native view on first read, so the Plugins page is usable. */
function nativeViewFor(homeId: string, profile: string, running: boolean): PluginView {
  if (!running) {
    // Mirror the backend's offline branch: read-only, list from the profile's
    // files (best-effort, includes core entries), distinct reason.
    return {
      mode: 'not-running',
      reason:
        '实例未在运行：启动该实例后即可在此管理插件；下面是该 Profile 的插件（只读，从 Profile 文件读出）。',
      files: true,
      plugins: [
        // Core entries are shown offline too (the old reader's blanket
        // @deepseek-ai/* filter is gone), sourced from the profile files.
        nativePlugin('dsh-base', '@deepseek-ai/dsh-base', 'DSH 基础', true, null, null),
        nativePlugin('dsh-auxiliary', '@dsh-plugin/dsh-auxiliary', '辅助工具', true, null, null),
        nativePlugin('ds-bot', '@canglongcl/ds-bot', 'ds-bot', false, null, null),
      ],
      bundles: [],
    }
  }
  const key = `${homeId}:${profile}`
  if (!mockNativeStore[key]) {
    mockNativeStore[key] = {
      mode: 'managed',
      reason: null,
      files: false,
      plugins: [
        // Core entries the launcher's old reader dropped: they must show up.
        nativePlugin(
          'include:plugin-manager',
          '@deepseek-ai/dsh-plugin-manager',
          '插件管理',
          true,
          null,
          'management-required',
        ),
        nativePlugin(
          'include:llm',
          '@deepseek-ai/dsh-llm',
          'LLM',
          true,
          'p1',
          null,
        ),
        nativePlugin(
          'include:plugin:dsh-auxiliary',
          '@dsh-plugin/dsh-auxiliary',
          '辅助工具',
          true,
          'p2',
          null,
        ),
        nativePlugin(
          'include:plugin:dsh-better-sidebar',
          'dsh-better-sidebar',
          '侧边栏增强',
          false,
          'p3',
          null,
        ),
      ],
      bundles: [
        {
          name: '@deepseek-ai/dsh-base',
          title: 'DSH 基础',
          description: null,
          version: '0.2.1-alpha.2',
          enabled: true,
          installed: true,
          removable: false,
          optional: false,
          availability: 'installation',
          readOnlyReason: null,
          errorCode: null,
        },
        {
          name: '@deepseek-ai/dsh-web-app',
          title: 'DSH 网页端',
          description: null,
          version: '0.2.1-alpha.2',
          enabled: true,
          installed: true,
          removable: false,
          optional: false,
          availability: 'installation',
          readOnlyReason: null,
          errorCode: null,
        },
        {
          name: '@canglongcl/ds-bot',
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
  return mockNativeStore[key]
}

function mockNewId(prefix: string): string {
  return `${prefix}-${uuid()}`
}

export async function mockCall<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const db = loadDb()
  function fail(msg: string): never {
    throw new Error(msg)
  }

  switch (cmd) {
    case 'list_homes':
      return db.homes as T
    case 'default_dedicated_home_path': {
      const name = String(args?.name ?? 'instance')
      const safe = name.replace(/[^\w一-龥.-]+/g, '_')
      return `C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\homes\\${safe}` as T
    }
    case 'create_home': {
      const name = String(args?.name ?? '').trim()
      const path = String(args?.path ?? '').trim()
      if (!name || !path) fail('名称与路径不能为空')
      const home: DshHome = { id: `h-${uuid()}`, name, path }
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
    case 'list_versions':
      return db.versions as T
    case 'fetch_available_versions':
      return [
        { version: '0.1.2-alpha.1', released_at: '2026-10-01T08:00:00Z', source: 'github' },
        { version: '0.1.0-rc.6', released_at: '2026-08-01T12:00:00Z' },
        { version: '0.1.0-rc.5', released_at: '2026-07-15T09:30:00Z' },
        { version: '0.1.0-rc.4', released_at: '2026-07-01T10:00:00Z' },
        { version: '0.1.0-rc.3', released_at: '2026-06-15T08:00:00Z' },
      ] as T
    case 'start_install_version_task': {
      const version = String(args?.version ?? '').trim()
      return mockCall('start_create_instance_task', { name: version, version, home_id: null, dedicated: true })
    }
    case 'start_create_instance_task': {
      const name = String(args?.name ?? '').trim()
      const version = String(args?.version ?? '').trim()
      const dedicated = Boolean(args?.dedicated)
      const homeIdArg = args?.home_id as string | null
      if (!name) fail('实例名称不能为空')
      if (!version) fail('版本号不能为空')
      if (db.instances.some((i) => i.name === name)) fail('同名实例已存在')
      if ([...mockTasks.values()].some((t) => t.state === 'running' && t.instance_name === name)) {
        fail('同名实例的下载任务已在进行中')
      }

      // Dedicated HOME is only materialized when the task finishes, mirroring
      // the real backend's placeholder semantics.
      let dedicatedPath: string | null = null
      if (dedicated) {
        const safe = name.replace(/[^\w一-龥.-]+/g, '_')
        dedicatedPath = `C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\homes\\${safe}`
      }
      if (!dedicated && (!homeIdArg || !db.homes.some((h) => h.id === homeIdArg))) {
        fail('请选择 DSH_HOME')
      }

      const task: TaskInfo = {
        id: mockNewId('t'),
        kind: 'create-instance',
        label: `下载 DSH ${version} 并创建实例「${name}」`,
        version,
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
          const existing = cur.homes.find(
            (h) => h.path.replace(/\\/g, '/').toLowerCase() === dedicatedPath!.replace(/\\/g, '/').toLowerCase(),
          )
          if (existing) {
            resolvedHomeId = existing.id
          } else {
            const home: DshHome = { id: mockNewId('h'), name, path: dedicatedPath! }
            cur.homes.push(home)
            resolvedHomeId = home.id
          }
        }
        // Install version record if missing.
        let ver = cur.versions.find((v) => v.version === version)
        if (!ver) {
          ver = {
            id: mockNewId('v'),
            version,
            dir: `C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\versions\\${version}`,
            dedicated_home: `C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher\\homes\\${version}`,
          }
          cur.versions.push(ver)
        }
        const inst: DshInstance = {
          id: mockNewId('i'),
          name,
          version_id: ver.id,
          home_id: resolvedHomeId!,
          env_overrides: {},
          default_profile: null,
          last_profile: null,
          preserve_symlinks: false,
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
      // Browser preview: assume Node + pnpm are available so the UI is usable.
      const mockRuntime: RuntimeStatus = {
        node: { installed: true, version: 'v22.14.0', path: '/usr/local/bin/node' },
        pnpm: { installed: true, version: '11.17.0', path: '/usr/local/bin/pnpm' },
        managers: ['brew'],
        node_below_recommended: false,
        requirements: {
          min_pnpm_major: 11,
          recommended_node_major: 20,
          pnpm_install_command: 'npm install -g pnpm',
          node_install_command: 'brew install node',
        },
      }
      return mockRuntime as T
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
      if (db.instances.some((i) => i.version_id === id)) fail('该版本仍被实例引用，无法删除')
      db.versions = db.versions.filter((v) => v.id !== id)
      saveDb(db)
      return undefined as T
    }
    case 'list_instances':
      return db.instances as T
    case 'update_instance': {
      const input = args?.input as DshInstance
      if (db.instances.some((i) => i.name === input.name && i.id !== input.id)) fail('同名实例已存在')
      db.instances = db.instances.map((i) => (i.id === input.id ? input : i))
      saveDb(db)
      return input as T
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
      const home = db.homes.find((h) => h.id === homeId)
      if (!home) fail('DSH_HOME 不存在')
      // Mock: combine the default set with previously created profiles.
      const base: string[] = home.path.endsWith('.dsh') ? ['web', 'demo', 'pack'] : ['web']
      const extras = mockProfiles[homeId] ?? []
      return [...base, ...extras] as T
    }
    case 'create_profile': {
      const homeId = String(args?.home_id)
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
      setTimeout(() => {
        const cur = loadDb()
        if (cur.running[id]?.state !== 'starting') return
        const running: InstanceStatus = {
          id,
          state: 'running',
          url: `http://127.0.0.1:${30000 + Math.floor(Math.random() * 20000)}`,
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
    case 'open_instance_directory':
    case 'open_home_directory':
      // Browser preview has no file manager; the target path is reported as-is.
      return 'C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher' as T
    case 'get_launcher_directory':
      return 'C:\\Users\\Administrator\\AppData\\Roaming\\in.dsh-plug.dsh-launcher' as T
    case 'pending_deep_link':
      return null as T
    case 'set_instance_icon':
    case 'clear_instance_icon':
      return undefined as T
    case 'read_instance_icon':
      return null as T
    case 'open_instance_terminal':
      return `DSH ${String(args?.instanceId ?? args?.instance_id ?? '')}` as T
    case 'list_instance_status':
      return Object.values(db.running) as T
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
      // HOME is running this profile (the launcher resolves HOME+Profile to an
      // instance), otherwise the read-only not-running stub.
      const running = db.instances.some(
        (i) =>
          i.home_id === homeId &&
          db.running[i.id]?.state === 'running' &&
          db.running[i.id]?.profile === profile,
      )
      return nativeViewFor(homeId, profile, running) as T
    }
    case 'set_bundle_enabled': {
      const input = args?.input as SetBundleEnabledInput
      const homeId = input.homeId
      const profile = input.profile
      const running = db.instances.some(
        (i) =>
          i.home_id === homeId &&
          db.running[i.id]?.state === 'running' &&
          db.running[i.id]?.profile === profile,
      )
      // No instance → a plain error (mirrors the backend Err), not a refusal.
      if (!running) fail('实例未在运行，请先启动实例')
      const view = nativeViewFor(homeId, profile, true)
      const bundle = view.bundles.find((b) => b.name === input.name)
      const refusal = (errorCode: string): ChangeOutcome => ({
        changed: false,
        application: 'failed',
        stage: 'enable',
        target: input.name,
        errorCode,
        diagnostic: null,
        warnings: [],
      })
      if (!bundle) return refusal('实例里找不到这个 Bundle') as T
      // A missing package's files cannot be read, so the layer cannot mount.
      // `removable:false` does NOT block this toggle — it only forbids deletion
      // (the delete rule is a separate concern).
      if (bundle.availability === 'missing') return refusal('实例里找不到这个 Bundle') as T
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
    case 'list_installed_plugins': {
      const homeId = String(args?.home_id)
      const profile = String(args?.profile ?? 'web')
      const key = `${homeId}:${profile}`
      // Seed a realistic set on first read so the Plugins page is usable.
      if (!mockPluginStore[key]) {
        mockPluginStore[key] = [
          { id: '@dsh-plugin/dsh-auxiliary', version: '^0.3.1', enabled: true, cordis_id: 'dsh-auxiliary' },
          { id: 'dsh-better-sidebar', version: '^1.2.0', enabled: false, cordis_id: 'dsh-better-sidebar' },
          { id: '@canglongcl/dsh-web-review', version: '^0.9.4', enabled: true, cordis_id: 'dsh-web-review' },
        ]
      }
      return mockPluginStore[key] as T
    }
    case 'set_plugins_enabled': {
      const input = args?.input as SetPluginsEnabledInput
      const key = `${input.homeId}:${input.profile}`
      const list = mockPluginStore[key] ?? []
      for (const p of list) {
        if (input.pluginIds.includes(p.id)) p.enabled = input.enabled
      }
      mockPluginStore[key] = list
      return undefined as T
    }
    case 'uninstall_plugin': {
      const input = args?.input as UninstallPluginInput
      const key = `${input.homeId}:${input.profile}`
      mockPluginStore[key] = (mockPluginStore[key] ?? []).filter((p) => p.id !== input.pluginId)
      return undefined as T
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

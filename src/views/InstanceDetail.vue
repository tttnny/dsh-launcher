<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { LOCAL_HOST_ID } from '@/api/types'
import type { DshInstance, DoctorReport, InstanceLocation } from '@/api/types'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import { useCopy } from '@/composables/useCopy'
import { useProfiles } from '@/composables/useProfiles'
import DlIcon from '@/components/DlIcon.vue'
import InstanceAvatar from '@/components/InstanceAvatar.vue'
import StatusBadge from '@/components/StatusBadge.vue'
import EmptyState from '@/components/EmptyState.vue'
import ProfilePickerDialog from '@/components/ProfilePickerDialog.vue'
import PluginPanel from '@/components/PluginPanel.vue'

const route = useRoute()
const router = useRouter()
const store = useLauncherStore()
const { copy } = useCopy()
const { loadForHome, forgetInstance } = useProfiles()

const id = computed(() => String(route.params.id ?? ''))
const inst = computed(() => store.instanceById(id.value))
const status = computed(() => store.statusOf(id.value))

const host = computed(() =>
  store.hostById(inst.value?.host_id ?? LOCAL_HOST_ID),
)
const version = computed(() =>
  inst.value ? store.versionById(inst.value.version_id) : undefined,
)
const home = computed(() =>
  inst.value ? store.homeById(inst.value.home_id) : undefined,
)

const hostName = computed(() => host.value?.display_name ?? '本机')
const versionName = computed(() => version.value?.version ?? '未知版本')
const homeName = computed(() => home.value?.name ?? '未知数据目录')
const homePath = computed(() => home.value?.path ?? '')
const isRemote = computed(() => !!host.value && !host.value.builtin)

/** 状态先于装饰：不可达时把这台机器给出的原因说清楚。 */
const statusTitle = computed(() =>
  status.value.state === 'unreachable'
    ? (host.value?.status?.message ?? '主机没有回应')
    : '',
)

// --- 配置列表（概览与诊断用） ------------------------------------------------

const homeProfiles = ref<string[]>([])
const homeProfilesError = ref('')

async function loadHomeProfiles(homeId: string) {
  if (!homeId) return
  const list = await loadForHome(homeId)
  if (list) {
    homeProfiles.value = list
    homeProfilesError.value = ''
  } else {
    homeProfilesError.value = '读不到这个数据目录的配置'
  }
}

watch(
  () => inst.value?.home_id,
  (h) => {
    if (h) void loadHomeProfiles(h)
  },
  { immediate: true },
)

const activeProfile = computed(
  () =>
    status.value.profile ??
    inst.value?.last_profile ??
    inst.value?.default_profile ??
    '',
)

// --- 标签页 ------------------------------------------------------------------

const TABS = [
  { key: 'overview', label: '概览' },
  { key: 'settings', label: '设置' },
  { key: 'plugins', label: '插件' },
  { key: 'diagnostics', label: '诊断' },
] as const
type TabKey = (typeof TABS)[number]['key']
const tab = ref<TabKey>('overview')

// --- 顶栏与概览动作 ----------------------------------------------------------

const openAction = useAction(() => api.openInstanceWindow(id.value), {
  success: () => '已在浏览器打开实例',
})
const stopAction = useAction(() => api.stopInstance(id.value), {
  success: () => '实例已停止',
})
const restartAction = useAction(() => api.restartInstance(id.value), {
  success: () => '实例已重启',
})
const retryAction = useAction(async () => {
  await api.refreshInstanceStatus(id.value)
  store.hosts = await api.listHosts()
  return true
}, {})

const terminalAction = useAction(() => api.openInstanceTerminal(id.value), {
  success: (label) => `已打开终端：${label}`,
})
const logAction = useAction(() => api.openInstanceLog(id.value), {
  success: (path) => `已在访达中定位日志：${path}`,
})
const dirAction = useAction(() => api.openInstanceDirectory(id.value), {
  success: () => '已在访达中打开数据目录',
})

const remoteLocation = ref<InstanceLocation | null>(null)
async function ensureRemoteLocation(): Promise<InstanceLocation> {
  if (!remoteLocation.value)
    remoteLocation.value = await api.instanceRemoteLocation(id.value)
  return remoteLocation.value
}
const copyRemotePathAction = useAction(async () => {
  const loc = await ensureRemoteLocation()
  await copy(loc.path, '远端路径已复制')
  return loc.path
})
const copyRemoteCommandAction = useAction(async () => {
  const loc = await ensureRemoteLocation()
  await copy(loc.command, '远端命令已复制')
  return loc.command
})

function copyUrl() {
  if (status.value.url) void copy(status.value.url, '地址已复制')
}

function retryStatus() {
  void retryAction.run()
}

function goBack() {
  void router.push({ name: 'instances' })
}

// --- 启动：先决定用哪个 Profile ---------------------------------------------

const pickerVisible = ref(false)
const pickerProfiles = ref<string[]>([])

const startAction = useAction(
  async (profile: string) => {
    await api.startInstance(id.value, profile)
    return profile
  },
  { success: () => '实例已启动' },
)

async function requestStart() {
  const i = inst.value
  if (!i) return
  const list = homeProfiles.value.length
    ? homeProfiles.value
    : await loadForHome(i.home_id)
  if (!list) {
    Message.error('读不到这个数据目录的配置，无法选择启动用的 Profile')
    return
  }
  if (list.length === 0) {
    Message.error('这个数据目录还没有 Profile，先在「数据」页创建配置')
    return
  }
  const preferred =
    (i.last_profile && list.includes(i.last_profile) && i.last_profile) ||
    (i.default_profile &&
      list.includes(i.default_profile) &&
      i.default_profile) ||
    ''
  if (preferred) {
    await startAction.run(preferred)
    return
  }
  if (list.length === 1) {
    await startAction.run(list[0])
    return
  }
  // 多个配置、没有默认也没有上次使用记录：问清楚这次用哪一个。
  pickerProfiles.value = list
  pickerVisible.value = true
}

function onPickProfile(profile: string) {
  void startAction.run(profile)
}

// --- 设置表单 ---------------------------------------------------------------

interface EnvRow {
  key: string
  value: string
}

const form = reactive<{
  name: string
  versionId: string
  homeId: string
  defaultProfile: string | undefined
  portText: string
  envRows: EnvRow[]
}>({
  name: '',
  versionId: '',
  homeId: '',
  defaultProfile: undefined,
  portText: '',
  envRows: [],
})

const baseline = ref('')
const formProfiles = ref<string[]>([])
const formProfilesError = ref('')
const formProfilesLoading = ref(false)

const hostVersions = computed(() => {
  const hid = inst.value?.host_id ?? LOCAL_HOST_ID
  return store.versionsByHost[hid] ?? []
})
const hostHomes = computed(() => {
  const hid = inst.value?.host_id ?? LOCAL_HOST_ID
  return store.homesByHost[hid] ?? []
})

async function loadFormProfiles(homeId: string) {
  if (!homeId) {
    formProfiles.value = []
    return
  }
  formProfilesLoading.value = true
  const list = await loadForHome(homeId)
  formProfilesLoading.value = false
  if (list) {
    formProfiles.value = list
    formProfilesError.value = ''
  } else {
    formProfiles.value = []
    formProfilesError.value = '读不到这个数据目录的配置'
  }
}

const ENV_KEY_RE = /^[A-Za-z_][A-Za-z0-9_]*$/

function envKeyError(row: EnvRow): string | null {
  const key = row.key.trim()
  if (!key) return null
  if (key === 'DSH_HOME') return 'DSH_HOME 由启动器管理，不可覆写'
  if (!ENV_KEY_RE.test(key)) return '变量名需匹配 [A-Za-z_][A-Za-z0-9_]*'
  return null
}

const envValid = computed(() => form.envRows.every((row) => !envKeyError(row)))

function envObject(rows: EnvRow[]): Record<string, string> {
  const out: Record<string, string> = {}
  for (const row of rows) {
    const key = row.key.trim()
    if (key) out[key] = row.value
  }
  return out
}

function sortedEnv(rows: EnvRow[]): [string, string][] {
  return Object.entries(envObject(rows)).sort(([a], [b]) => a.localeCompare(b))
}

function snapshot(): string {
  return JSON.stringify({
    name: form.name.trim(),
    versionId: form.versionId,
    homeId: form.homeId,
    defaultProfile: form.defaultProfile || null,
    port: form.portText.trim(),
    env: sortedEnv(form.envRows),
  })
}

const dirty = computed(() => snapshot() !== baseline.value)

function onFormHomePicked() {
  form.defaultProfile = undefined
  void loadFormProfiles(form.homeId)
}

function resetForm() {
  const i = inst.value
  if (!i) return
  form.name = i.name
  form.versionId = i.version_id
  form.homeId = i.home_id
  form.defaultProfile = i.default_profile ?? undefined
  form.portText = i.port != null ? String(i.port) : ''
  form.envRows = Object.entries(i.env_overrides ?? {}).map(([key, value]) => ({
    key,
    value,
  }))
  baseline.value = snapshot()
  void loadFormProfiles(form.homeId)
}

watch(
  () => inst.value?.id,
  (nid) => {
    if (nid) resetForm()
  },
  { immediate: true },
)

watch(
  () => form.homeId,
  (h) => {
    if (h) void loadFormProfiles(h)
  },
)

function parsePort(text: string): number | null | 'invalid' {
  const t = text.trim()
  if (!t) return null
  const n = Number(t)
  if (!Number.isInteger(n) || n < 1 || n > 65535) return 'invalid'
  return n
}

const saveAction = useAction(
  async () => {
    const i = inst.value
    if (!i) return undefined
    const name = form.name.trim()
    if (!name) throw new Error('实例名称不能为空')
    if (!envValid.value) throw new Error('环境变量名不合法，请检查后再保存')
    const port = parsePort(form.portText)
    if (port === 'invalid')
      throw new Error('端口需在 1–65535 之间，留空表示自动分配')
    const input: DshInstance = {
      ...i,
      name,
      version_id: form.versionId,
      home_id: form.homeId,
      default_profile: form.defaultProfile || null,
      port,
      env_overrides: envObject(form.envRows),
    }
    const updated = await api.updateInstance(input, i.host_id)
    await store.refreshInstances()
    return updated
  },
  { success: () => '设置已保存' },
)

async function save() {
  if (!dirty.value) return
  const updated = await saveAction.run()
  if (updated) baseline.value = snapshot()
}

function restore() {
  resetForm()
}

function addEnvRow() {
  form.envRows.push({ key: '', value: '' })
}

function removeEnvRow(index: number) {
  form.envRows.splice(index, 1)
}

// --- 图标 -------------------------------------------------------------------

const iconSource = ref('')
const applyIconAction = useAction(
  async () => {
    const source = iconSource.value.trim()
    if (!source) throw new Error('请填写图片路径或网址')
    await api.setInstanceIcon(id.value, source)
    await store.refreshInstances()
    return true
  },
  { success: () => '图标已更新' },
)
async function applyIcon() {
  const ok = await applyIconAction.run()
  if (ok) iconSource.value = ''
}

async function pickIconFile() {
  if (!api.isTauri) {
    Message.info('浏览器预览不支持系统文件选择，请填写图片路径')
    return
  }
  const { open } = await import('@tauri-apps/plugin-dialog')
  const file = await open({
    multiple: false,
    filters: [
      {
        name: '图片',
        extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'],
      },
    ],
  })
  if (typeof file !== 'string') return
  iconSource.value = file
  await applyIcon()
}
const clearIconAction = useAction(
  async () => {
    await api.clearInstanceIcon(id.value)
    await store.refreshInstances()
    return true
  },
  { success: () => '已恢复默认图标' },
)

// --- 诊断 -------------------------------------------------------------------

const report = ref<DoctorReport | null>(null)
const diagLoading = ref(false)
const diagError = ref('')

async function runDiagnostics() {
  const i = inst.value
  if (!i) return
  const profile = activeProfile.value || homeProfiles.value[0] || ''
  if (!profile) {
    diagError.value = '这个实例还没有可用的 Profile，先在「数据」页创建配置'
    report.value = null
    return
  }
  diagLoading.value = true
  diagError.value = ''
  try {
    report.value = await api.checkInstanceHealth(i.id, profile)
  } catch (e) {
    diagError.value = String(e)
    report.value = null
  } finally {
    diagLoading.value = false
  }
}

watch(tab, (t) => {
  if (
    t === 'diagnostics' &&
    !report.value &&
    !diagError.value &&
    !diagLoading.value
  )
    void runDiagnostics()
})

// --- 删除 -------------------------------------------------------------------

const deleteAction = useAction(async () => {
  await api.deleteInstance(id.value, inst.value?.host_id)
  forgetInstance(id.value)
  await store.refreshInstances()
  return true
})

function confirmDelete() {
  const i = inst.value
  if (!i) return
  Modal.confirm({
    title: `删除实例「${i.name}」？`,
    content: `只删除启动器里的这条实例记录：DSH 版本和数据目录「${homeName.value}」都会保留，磁盘上的数据不动。`,
    okText: '删除',
    cancelText: '取消',
    okButtonProps: { status: 'danger' },
    onOk: async () => {
      const ok = await deleteAction.run()
      if (ok !== true) return
      Message.success(`已删除实例「${i.name}」`)
      goBack()
    },
  })
}
</script>

<template>
  <div class="page detail-page">
    <template v-if="inst">
      <header class="page-head detail-head">
        <div class="page-head-main">
          <button class="page-back" type="button" @click="goBack">
            <DlIcon name="arrow-left" :size="13" />
            返回实例列表
          </button>
          <div class="detail-id">
            <InstanceAvatar :id="inst.id" :icon="inst.icon" :size="46" />
            <div class="detail-id-text">
              <div class="detail-name-row">
                <h1 class="page-title">{{ inst.name }}</h1>
                <StatusBadge :state="status.state" :title="statusTitle" />
              </div>
              <div class="detail-meta">
                <span>{{ hostName }}</span>
                <span class="meta-sep">·</span>
                <span class="mono">{{ versionName }}</span>
                <span class="meta-sep">·</span>
                <span>{{ homeName }}</span>
              </div>
            </div>
          </div>
        </div>

        <div class="page-actions detail-actions">
          <template v-if="status.state === 'running'">
            <button
              class="btn btn--run"
              :disabled="openAction.busy['*']"
              @click="openAction.run()"
            >
              <DlIcon name="external" :size="14" />
              打开
            </button>
            <button
              class="btn btn--quiet"
              :disabled="stopAction.busy['*']"
              @click="stopAction.run()"
            >
              <DlIcon name="stop" :size="13" />
              停止
            </button>
            <button
              class="btn btn--quiet"
              :disabled="restartAction.busy['*']"
              @click="restartAction.run()"
            >
              <DlIcon name="restart" :size="13" />
              重启
            </button>
          </template>
          <template v-else-if="status.state === 'starting'">
            <button class="btn btn--quiet" disabled>
              <span class="spinner" />
              启动中
            </button>
            <button
              class="btn btn--quiet"
              :disabled="stopAction.busy['*']"
              @click="stopAction.run()"
            >
              <DlIcon name="stop" :size="13" />
              停止
            </button>
          </template>
          <template v-else-if="status.state === 'unreachable'">
            <button
              class="btn btn--quiet"
              :disabled="retryAction.busy['*']"
              @click="retryStatus"
            >
              <DlIcon name="refresh" :size="14" />
              重试
            </button>
          </template>
          <template v-else>
            <button
              class="btn btn--primary"
              :disabled="startAction.busy['*']"
              @click="requestStart"
            >
              <DlIcon name="play" :size="13" />
              启动
            </button>
          </template>

          <a-dropdown trigger="click" position="br">
            <button class="icon-btn" title="更多操作">
              <DlIcon name="more" :size="16" />
            </button>
            <template #content>
              <a-doption @click="terminalAction.run()">
                <DlIcon name="terminal" :size="13" />
                打开终端
              </a-doption>
              <a-doption @click="logAction.run()">
                <DlIcon name="log" :size="13" />
                查看日志
              </a-doption>
              <a-doption v-if="status.url" @click="copyUrl">
                <DlIcon name="copy" :size="13" />
                复制地址
              </a-doption>
              <a-doption v-if="!isRemote" @click="dirAction.run()">
                <DlIcon name="folder" :size="13" />
                打开数据目录
              </a-doption>
              <template v-else>
                <a-doption @click="copyRemotePathAction.run()">
                  <DlIcon name="copy" :size="13" />
                  复制远端路径
                </a-doption>
                <a-doption @click="copyRemoteCommandAction.run()">
                  <DlIcon name="terminal" :size="13" />
                  复制远端命令
                </a-doption>
              </template>
              <a-doption @click="tab = 'settings'">
                <DlIcon name="settings" :size="13" />
                实例设置
              </a-doption>
              <a-doption @click="confirmDelete">
                <span class="menu-danger">
                  <DlIcon name="trash" :size="13" />
                  删除实例
                </span>
              </a-doption>
            </template>
          </a-dropdown>
        </div>
      </header>

      <nav class="tabs">
        <button
          v-for="t in TABS"
          :key="t.key"
          type="button"
          class="tab"
          :class="{ 'is-active': tab === t.key }"
          @click="tab = t.key"
        >
          {{ t.label }}
        </button>
      </nav>

      <!-- 概览 -->
      <div v-if="tab === 'overview'" class="detail-body">
        <div v-if="status.state === 'unreachable'" class="notice notice--warn">
          <DlIcon name="alert" :size="14" />
          <div class="notice-body">
            <div class="notice-title">主机现在连不上</div>
            <div>
              {{
                statusTitle ||
                '读不到这个实例的运行状态。等这台机器恢复连接后点「重试」。'
              }}
            </div>
          </div>
          <button
            class="btn btn--quiet btn--sm"
            :disabled="retryAction.busy['*']"
            @click="retryStatus"
          >
            重试
          </button>
        </div>

        <section class="section">
          <div class="section-head">
            <span class="section-title">运行状态</span>
          </div>
          <div class="surface">
            <div class="field-row">
              <div class="field-row-info">
                <span class="field-label">地址</span>
                <span class="field-hint"
                  >在这个 Mac 上打开实例的浏览器地址</span
                >
              </div>
              <div class="field-value">
                <template v-if="status.url">
                  <span class="mono addr">{{ status.url }}</span>
                  <button class="icon-btn" title="复制地址" @click="copyUrl">
                    <DlIcon name="copy" :size="13" />
                  </button>
                  <button
                    class="icon-btn"
                    title="打开"
                    @click="openAction.run()"
                  >
                    <DlIcon name="external" :size="14" />
                  </button>
                </template>
                <span v-else class="muted">—</span>
              </div>
            </div>
            <div class="field-row">
              <div class="field-row-info">
                <span class="field-label">端口</span>
                <span class="field-hint">实例优先使用的网页端口</span>
              </div>
              <div class="field-value">
                <span class="mono">{{ inst.port ?? '自动分配' }}</span>
              </div>
            </div>
            <div class="field-row">
              <div class="field-row-info">
                <span class="field-label">Profile</span>
                <span class="field-hint">这次运行使用的配置</span>
              </div>
              <div class="field-value">
                <span class="mono">{{ activeProfile || '未选择' }}</span>
              </div>
            </div>
            <div class="field-row">
              <div class="field-row-info">
                <span class="field-label">机器</span>
              </div>
              <div class="field-value">
                <span>{{ hostName }}</span>
                <span v-if="isRemote && host?.ssh_target" class="mono dim">
                  {{ host.ssh_target }}
                </span>
              </div>
            </div>
            <div class="field-row">
              <div class="field-row-info">
                <span class="field-label">版本</span>
              </div>
              <div class="field-value">
                <span class="mono">{{ versionName }}</span>
              </div>
            </div>
            <div class="field-row">
              <div class="field-row-info">
                <span class="field-label">数据目录</span>
                <span class="field-hint">{{ homeName }}</span>
              </div>
              <div class="field-value">
                <span class="mono path">{{ homePath }}</span>
              </div>
            </div>
          </div>
        </section>

        <section class="section">
          <div class="section-head">
            <span class="section-title">机器操作</span>
          </div>
          <div class="btn-row">
            <button
              class="btn btn--quiet"
              :disabled="terminalAction.busy['*']"
              @click="terminalAction.run()"
            >
              <DlIcon name="terminal" :size="14" />
              打开终端
            </button>
            <button
              class="btn btn--quiet"
              :disabled="logAction.busy['*']"
              @click="logAction.run()"
            >
              <DlIcon name="log" :size="14" />
              查看日志
            </button>
            <button
              v-if="!isRemote"
              class="btn btn--quiet"
              :disabled="dirAction.busy['*']"
              @click="dirAction.run()"
            >
              <DlIcon name="folder" :size="14" />
              打开数据目录
            </button>
            <template v-else>
              <button
                class="btn btn--quiet"
                :disabled="copyRemotePathAction.busy['*']"
                @click="copyRemotePathAction.run()"
              >
                <DlIcon name="copy" :size="14" />
                复制远端路径
              </button>
              <button
                class="btn btn--quiet"
                :disabled="copyRemoteCommandAction.busy['*']"
                @click="copyRemoteCommandAction.run()"
              >
                <DlIcon name="terminal" :size="14" />
                复制远端命令
              </button>
            </template>
          </div>
          <p v-if="homeProfilesError" class="detail-note">
            {{ homeProfilesError }}
          </p>
        </section>
      </div>

      <!-- 设置 -->
      <div v-else-if="tab === 'settings'" class="detail-body settings-body">
        <div class="surface">
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">名称</span>
              <span class="field-hint">在这个启动器里显示的名字</span>
            </div>
            <div class="field-value">
              <input
                v-model="form.name"
                class="input name-input"
                placeholder="实例名称"
              />
            </div>
          </div>

          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">图标</span>
              <span class="field-hint">填图片文件路径或 https 网址</span>
            </div>
            <div class="field-value icon-field">
              <InstanceAvatar :id="inst.id" :icon="inst.icon" :size="30" />
              <input
                v-model="iconSource"
                class="input input--mono icon-input"
                placeholder="图片路径或网址"
              />
              <button class="btn btn--quiet btn--sm" @click="pickIconFile">
                选择图片
              </button>
              <button
                class="btn btn--quiet btn--sm"
                :disabled="!iconSource.trim() || applyIconAction.busy['*']"
                @click="applyIcon"
              >
                应用
              </button>
              <button
                class="btn btn--ghost btn--sm"
                :disabled="clearIconAction.busy['*']"
                @click="clearIconAction.run()"
              >
                恢复默认
              </button>
            </div>
          </div>

          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">版本</span>
              <span class="field-hint">这台机器上已安装的 DSH 版本</span>
            </div>
            <div class="field-value">
              <a-select v-model="form.versionId" style="width: 240px">
                <a-option v-for="v in hostVersions" :key="v.id" :value="v.id">
                  {{ v.version }}
                </a-option>
              </a-select>
            </div>
          </div>

          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">DSH_HOME</span>
              <span class="field-hint">保存会话、凭据和配置的数据目录</span>
            </div>
            <div class="field-value">
              <a-select
                v-model="form.homeId"
                style="width: 300px"
                @change="onFormHomePicked"
              >
                <a-option v-for="h in hostHomes" :key="h.id" :value="h.id">
                  {{ h.name }} — {{ h.path }}
                </a-option>
              </a-select>
            </div>
          </div>

          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">默认 Profile</span>
              <span class="field-hint">启动时优先使用的配置</span>
            </div>
            <div class="field-value">
              <a-select
                v-model="form.defaultProfile"
                allow-clear
                placeholder="跟随上次使用"
                :loading="formProfilesLoading"
                style="width: 240px"
              >
                <a-option v-for="p in formProfiles" :key="p" :value="p">
                  {{ p }}
                </a-option>
              </a-select>
              <span v-if="formProfilesError" class="error-text">{{
                formProfilesError
              }}</span>
            </div>
          </div>

          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">端口</span>
              <span class="field-hint">留空表示每次自动选一个空闲端口</span>
            </div>
            <div class="field-value">
              <input
                v-model="form.portText"
                class="input input--mono port-input"
                inputmode="numeric"
                placeholder="自动分配"
              />
            </div>
          </div>

          <div class="field-row env-row">
            <div class="field-row-info">
              <span class="field-label">环境变量覆写</span>
              <span class="field-hint">启动时额外注入的变量，每行一个</span>
            </div>
            <div class="field-value env-field">
              <div
                v-for="(row, idx) in form.envRows"
                :key="idx"
                class="env-line"
              >
                <input
                  v-model="row.key"
                  class="input input--mono env-key"
                  :class="{ 'input--error': !!envKeyError(row) }"
                  placeholder="KEY"
                />
                <span class="env-eq">=</span>
                <input
                  v-model="row.value"
                  class="input input--mono env-value"
                  placeholder="VALUE"
                />
                <button
                  class="icon-btn"
                  title="删除这一行"
                  @click="removeEnvRow(idx)"
                >
                  <DlIcon name="close" :size="13" />
                </button>
                <span v-if="envKeyError(row)" class="error-text env-error">{{
                  envKeyError(row)
                }}</span>
              </div>
              <span v-if="!envValid" class="error-text"
                >先修正上面标红的环境变量名</span
              >
              <button class="btn btn--ghost btn--sm env-add" @click="addEnvRow">
                <DlIcon name="plus" :size="13" />
                添加变量
              </button>
            </div>
          </div>
        </div>

        <div class="save-bar" :class="{ 'is-dirty': dirty }">
          <span class="save-hint">{{
            dirty ? '有未保存的改动' : '没有改动'
          }}</span>
          <div class="save-actions">
            <button
              class="btn btn--quiet"
              :disabled="!dirty || saveAction.busy['*']"
              @click="restore"
            >
              还原
            </button>
            <button
              class="btn btn--primary"
              :disabled="!dirty || !envValid || saveAction.busy['*']"
              @click="save"
            >
              保存
            </button>
          </div>
        </div>

        <section class="section danger-section">
          <div class="section-head">
            <span class="section-title">删除实例</span>
          </div>
          <div class="surface danger-card">
            <div class="danger-text">
              <span class="field-label">删除这个实例</span>
              <span class="field-hint"
                >只删除启动器里的这条实例记录；DSH
                版本和数据目录都会保留，磁盘上的数据不动。</span
              >
            </div>
            <button class="btn btn--danger-solid" @click="confirmDelete">
              <DlIcon name="trash" :size="13" />
              删除实例
            </button>
          </div>
        </section>
      </div>

      <!-- 插件 -->
      <div v-else-if="tab === 'plugins'" class="detail-body">
        <PluginPanel :instance="inst" />
      </div>

      <!-- 诊断 -->
      <div v-else class="detail-body">
        <div class="section-head">
          <span class="section-title">依赖自检</span>
          <span class="section-hint">检查这个实例当前配置的依赖是否完整</span>
          <div class="section-actions">
            <button
              class="btn btn--quiet btn--sm"
              :disabled="diagLoading"
              @click="runDiagnostics"
            >
              <DlIcon name="refresh" :size="13" />
              重新检查
            </button>
          </div>
        </div>

        <div v-if="diagLoading" class="surface surface--pad muted">
          正在检查…
        </div>
        <div v-else-if="diagError" class="notice notice--danger">
          <DlIcon name="alert" :size="14" />
          <div class="notice-body">
            <div class="notice-title">自检未能完成</div>
            <div>{{ diagError }}</div>
          </div>
        </div>
        <template v-else-if="report">
          <div v-if="report.findings.length === 0" class="notice notice--ok">
            <DlIcon name="check" :size="14" />
            <div class="notice-body">
              <div class="notice-title">没有发现异常</div>
              <div>这个实例（{{ report.profile }}）的依赖完整。</div>
            </div>
          </div>
          <div v-else class="surface">
            <div
              v-for="(f, idx) in report.findings"
              :key="idx"
              class="list-row"
            >
              <span
                class="tag"
                :class="f.level === 'error' ? 'tag--danger' : 'tag--warn'"
              >
                {{ f.level === 'error' ? '错误' : '警告' }}
              </span>
              <div class="row-main">
                <span class="row-title finding-msg">{{ f.message }}</span>
                <span class="row-meta mono">{{ f.code }}</span>
              </div>
            </div>
          </div>
        </template>
        <div v-else class="surface surface--pad">
          <span class="muted">还没有运行自检。</span>
        </div>
      </div>
    </template>

    <div v-else-if="!store.loaded" class="surface surface--pad muted">
      正在读取实例…
    </div>
    <EmptyState
      v-else
      icon="instance"
      title="找不到这个实例"
      description="它可能已经被删除。返回实例列表看看还有哪些实例。"
    >
      <button class="btn btn--primary" @click="goBack">返回实例列表</button>
    </EmptyState>

    <ProfilePickerDialog
      v-model:visible="pickerVisible"
      :instance="inst ?? null"
      :profiles="pickerProfiles"
      @pick="onPickProfile"
    />
  </div>
</template>

<style lang="scss" scoped>
.detail-head {
  align-items: flex-start;
}

.detail-id {
  display: flex;
  align-items: center;
  gap: 14px;
  min-width: 0;
}

.detail-id-text {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.detail-name-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.detail-meta {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 7px;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.meta-sep {
  color: var(--dl-ink-4);
}

.detail-actions {
  padding-top: 2px;
}

.detail-body {
  display: flex;
  flex-direction: column;
  gap: 0;
}

.detail-note {
  margin: 10px 2px 0;
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

// 值一律靠右，长地址与路径截断而不换行。
.field-value {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  flex-wrap: wrap;
  gap: 6px;
  min-width: 0;
  max-width: 62%;
  text-align: right;
}

.addr {
  max-width: 420px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--dl-run);
}

.path {
  max-width: 460px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.name-input {
  width: 260px;
}

.port-input {
  width: 140px;
}

.icon-field {
  flex-wrap: nowrap;
  max-width: none;
}

.icon-input {
  width: 260px;
}

.env-row {
  align-items: flex-start;
}

.env-field {
  flex-direction: column;
  align-items: flex-end;
  gap: 8px;
  max-width: none;
  flex: 1;
}

.env-line {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 6px;
  width: 100%;
}

.env-key {
  width: 200px;
}

.env-error {
  flex-basis: 100%;
}

.env-eq {
  color: var(--dl-ink-4);
}

.env-value {
  width: 260px;
}

.env-add {
  align-self: flex-end;
}

.btn-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

// 底部的保存栏：只有脏了才亮起来，一直贴在可见区域底部。
.save-bar {
  position: sticky;
  bottom: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-top: 16px;
  padding: 12px 16px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-card);
  background: var(--dl-surface);
  box-shadow: var(--dl-lift-2);

  &.is-dirty {
    border-color: var(--dl-accent-line);
  }
}

.save-hint {
  color: var(--dl-ink-3);
  font-size: var(--dl-text-sm);
}

.save-bar.is-dirty .save-hint {
  color: var(--dl-accent-ink);
  font-weight: 600;
}

.save-actions {
  display: flex;
  gap: 8px;
}

.danger-section {
  margin-top: 34px;
}

.danger-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  padding: 16px;
}

.danger-text {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.menu-danger {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--dl-danger);
}

.finding-msg {
  font-size: var(--dl-text-md);
  font-weight: 550;
}
</style>

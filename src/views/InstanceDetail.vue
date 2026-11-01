<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { onBeforeRouteLeave, useRoute, useRouter } from 'vue-router'
import { Message, Modal } from '@arco-design/web-vue'
import { api } from '@/api'
import { useAction } from '@/composables/useAction'
import { useCopy } from '@/composables/useCopy'
import { usePluginLink } from '@/composables/usePluginLink'
import { useLauncherStore } from '@/stores/launcher'
import { LOCAL_HOST_ID } from '@/api/types'
import type { DshInstance } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'
import InstanceAvatar from '@/components/InstanceAvatar.vue'

// Everything about one instance. The fields are the binding (name, version,
// HOME) and the launch options (default profile, port), then the environment
// overrides; the danger zone removes the instance and nothing else — the
// version and the HOME are shared resources and outlive it.

const route = useRoute()
const router = useRouter()
const store = useLauncherStore()
const { canManagePlugins, openPlugins } = usePluginLink()

const id = computed(() => (route.params.id as string | undefined) ?? '')

const name = ref('')
const versionId = ref<string | undefined>(undefined)
const homeId = ref<string | undefined>(undefined)
const defaultProfile = ref<string | undefined>(undefined)
const profiles = ref<string[]>([])
/** Why the selected HOME's profiles could not be read; the page states it once. */
const profilesError = ref('')
const portInput = ref('')
const loaded = ref(false)
/** Set when the page navigates on its own, after saving or deleting. */
const departing = ref(false)

interface EnvRow {
  key: string
  value: string
}
const envRows = ref<EnvRow[]>([])

const ENV_KEY_RE = /^[A-Za-z_][A-Za-z0-9_]*$/
const RESERVED_KEYS = new Set(['DSH_HOME'])

function envKeyError(row: EnvRow): string | null {
  if (!row.key) return null
  if (RESERVED_KEYS.has(row.key)) return `DSH_HOME 由启动器管理，不可覆写`
  if (!ENV_KEY_RE.test(row.key)) return `需匹配 [A-Za-z_][A-Za-z0-9_]*`
  return null
}

const envValid = computed(() => envRows.value.every((r) => !envKeyError(r)))

onMounted(async () => {
  const inst =
    store.instanceById(id.value) ??
    (await api.listInstances(null, true)).find((i) => i.id === id.value)
  if (!inst) {
    Message.error(`实例不存在`)
    void router.replace({ name: 'instances' })
    return
  }
  fillForm(inst)
  loaded.value = true
  await nextTick()
  baseline.value = formSnapshot()
})

const current = computed(() => store.instanceById(id.value))
const status = computed(() => store.statusOf(id.value))

const STATUS_LABELS: Record<string, string> = {
  stopped: '已停止',
  starting: '启动中',
  running: '运行中',
  exited: '已退出',
  unreachable: '主机不可达',
}
/** Where a remote instance really is, as that machine printed it — stated next
 * to the forwarded address so the two are never read as one. */
const remoteUrl = computed(() => status.value.remote_url ?? null)
const running = computed(
  () => status.value.state === 'running' || status.value.state === 'starting',
)
/**
 * 「主机不可达」 — the machine did not answer, so the state is unknown. The page
 * offers the retry and never turns it into 已停止 (ADR-0007 后果).
 */
const unreachable = computed(() => status.value.state === 'unreachable')
const hostReason = computed(
  () => store.hostById(current.value?.host_id)?.status?.message ?? '',
)
const retryAction = useAction(
  async (instanceId: string) => {
    await api.refreshInstanceStatus(instanceId)
    store.hosts = await api.listHosts()
    return true
  },
  { key: (instanceId) => instanceId },
)
/** The selected DSH_HOME record, while the launcher still knows it. */
const homeRecord = computed(() => store.homeById(homeId.value ?? ''))
/** The selected DSH_HOME's name; an unresolved one reads as 未知 DSH_HOME
 * instead of its internal id. */
const homeLabel = computed(() =>
  homeRecord.value
    ? homeRecord.value.name
    : homeId.value
      ? '未知 DSH_HOME'
      : '—',
)
/** The DSH_HOME's full path, or its internal id when unresolved. Kept for the
 * option and the tooltip, not for the closed control's text. */
const homeTitle = computed(() => homeRecord.value?.path ?? homeId.value ?? '')
/** The selected version's display name; an unresolved one reads as 未知版本
 * instead of its internal id. */
const versionLabel = computed(() => {
  const known = store.versionById(versionId.value ?? '')
  return known ? known.version : versionId.value ? '未知版本' : '—'
})
/** The version's internal id, kept for the option and the tooltip. */
const versionTitle = computed(() => versionId.value ?? '')
/**
 * The machine this instance runs on. Its version and its DSH_HOMEs live there —
 * an instance is one binding on one host (ADR-0006 §1) — so the two selects below
 * list that machine's resources and nothing else: a combination spanning two
 * machines is not one the backend could accept.
 */
const hostId = computed(() => current.value?.host_id ?? LOCAL_HOST_ID)
const host = computed(() => store.hostById(hostId.value))
/** On another machine: its DSH_HOME is a directory there, not on this Mac. */
const remote = computed(() => !!host.value && !host.value.builtin)
const hostVersions = computed(() => store.versionsByHost[hostId.value] ?? [])
const hostHomes = computed(() => store.homesByHost[hostId.value] ?? [])
/** Why that machine's installed versions could not be read. */
const versionReadError = computed(() => store.versionErrors[hostId.value] ?? '')
/** Why that machine's DSH_HOMEs could not be read. */
const homeReadError = computed(() => store.homeErrors[hostId.value] ?? '')
/**
 * The one failure this page states: a resource it needs on that machine could
 * not be read. The form keeps the saved values on screen and says so once.
 */
const resourceError = computed(
  () => versionReadError.value || homeReadError.value || profilesError.value,
)

function retryHost() {
  void store.loadHostResources(hostId.value)
  void loadProfiles(homeId.value)
}

/**
 * The form's identity, used to tell "nothing to save" from "the user changed
 * something". Everything the Save button writes belongs here — a field missing
 * from it renders as an edit that cannot be saved.
 */
function formSnapshot(): string {
  return JSON.stringify([
    name.value.trim(),
    versionId.value ?? '',
    homeId.value ?? '',
    defaultProfile.value ?? '',
    portInput.value.trim(),
    envRows.value
      .filter((r) => r.key.trim() || r.value)
      .map((r) => [r.key.trim(), r.value])
      .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)),
  ])
}

const baseline = ref('')
/** Loads the instance's saved values into the form. */
function fillForm(inst: DshInstance) {
  name.value = inst.name
  versionId.value = inst.version_id
  homeId.value = inst.home_id
  defaultProfile.value = inst.default_profile ?? undefined
  portInput.value = inst.port ? String(inst.port) : ''
  envRows.value = Object.entries(inst.env_overrides).map(([key, value]) => ({
    key,
    value,
  }))
}

// --- Profiles of the selected HOME --------------------------------------------

/** Reads the selected HOME's profiles; a failed read keeps no list and records
 * why, so the page states it in one place. */
async function loadProfiles(homeId: string | undefined) {
  profiles.value = []
  profilesError.value = ''
  if (!homeId) return
  try {
    profiles.value = await api.listProfiles(homeId)
  } catch (e) {
    profilesError.value = String(e)
  }
}

watch(homeId, (v) => {
  void loadProfiles(v)
})

/**
 * Switching the HOME invalidates the chosen default profile. The clear lives on
 * the control, not in the watcher above: the form's own load must keep the
 * instance's saved default profile.
 */
function onHomePicked() {
  defaultProfile.value = undefined
}

// --- Save ---------------------------------------------------------------------

/** Empty means "random port"; anything else must be a real port number. */
function parsePort(raw: string): number | null {
  const text = raw.trim()
  if (!text) return null
  const n = Number(text)
  return Number.isInteger(n) && n >= 1 && n <= 65535 ? n : null
}

const portError = computed(() =>
  portInput.value.trim() && parsePort(portInput.value) === null
    ? `端口号需为 1-65535 的整数`
    : '',
)

const formValid = computed(
  () =>
    name.value.trim().length > 0 &&
    !!versionId.value &&
    !!homeId.value &&
    !portError.value &&
    envValid.value,
)

const saveAction = useAction(
  async () => {
    const inst = current.value
    if (!inst) return undefined
    const envOverrides: Record<string, string> = {}
    for (const row of envRows.value)
      if (row.key) envOverrides[row.key] = row.value
    const updated: DshInstance = {
      ...inst,
      name: name.value.trim(),
      version_id: versionId.value!,
      home_id: homeId.value!,
      env_overrides: envOverrides,
      default_profile: defaultProfile.value ?? null,
      port: parsePort(portInput.value),
    }
    return api.updateInstance(updated, inst.host_id)
  },
  { success: () => `实例已保存` },
)

const saving = computed(() => saveAction.busy['*'])
/** True once the form differs from what is saved on the instance. */
const dirty = computed(
  () =>
    loaded.value && baseline.value !== '' && formSnapshot() !== baseline.value,
)

async function onSave() {
  if (!formValid.value) return
  const ok = await saveAction.run()
  if (!ok) return
  // The record is already saved: leaving must not be blocked by the unsaved
  // guard, nor by a failed list refresh.
  departing.value = true
  try {
    await store.refreshInstances()
  } catch {
    // A failed refresh only leaves the board stale; the save itself stands.
  }
  void router.push({ name: 'instances' })
}

/** 还原修改 puts every field back to the saved instance and keeps the page open;
 * going back is the header's job. */
function restoreChanges() {
  const inst = current.value
  if (!inst) return
  fillForm(inst)
  void nextTick(() => (baseline.value = formSnapshot()))
}

// A page left by another route must not drop unsaved edits without asking.
onBeforeRouteLeave((_to, _from, next) => {
  if (departing.value || !dirty.value) {
    next()
    return
  }
  Modal.confirm({
    title: '放弃修改？',
    content: '这个实例还有没保存的修改，离开后这些修改会丢失。',
    okText: '放弃修改',
    cancelText: '继续编辑',
    modalClass: 'dl-sheet',
    closable: false,
    maskClosable: false,
    escToClose: false,
    onOk: () => next(),
    onCancel: () => next(false),
  })
})

// --- Icon ---------------------------------------------------------------------

const iconUrl = ref<string | null>(null)
const iconInput = ref('')

async function loadIcon() {
  try {
    iconUrl.value = await api.readInstanceIcon(id.value)
  } catch {
    iconUrl.value = null
  }
}
onMounted(loadIcon)

const iconAction = useAction(
  async (source: string) => {
    await api.setInstanceIcon(id.value, source)
    return true
  },
  { success: () => `实例图标已更新` },
)

const clearIconAction = useAction(async () => {
  await api.clearInstanceIcon(id.value)
  return true
})

async function applyIconInput() {
  if (!iconInput.value.trim()) return
  const ok = await iconAction.run(iconInput.value.trim())
  if (ok !== true) return
  iconInput.value = ''
  await loadIcon()
  await store.refreshInstances()
}

async function pickIconFile() {
  const { open } = await import('@tauri-apps/plugin-dialog')
  const file = await open({
    multiple: false,
    filters: [
      {
        name: 'Image',
        extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'],
      },
    ],
  })
  if (typeof file !== 'string') return
  const ok = await iconAction.run(file)
  if (ok !== true) return
  await loadIcon()
  await store.refreshInstances()
}

async function clearIcon() {
  const ok = await clearIconAction.run()
  if (ok !== true) return
  await loadIcon()
  await store.refreshInstances()
}

// --- Quick links --------------------------------------------------------------

const dirAction = useAction(() => api.openInstanceDirectory(id.value), {
  success: (path) => `已在访达中打开目录：${path}`,
})
const logAction = useAction(() => api.openInstanceLog(id.value), {
  success: (path) => `已在访达中定位日志：${path}`,
})
const terminalAction = useAction(() => api.openInstanceTerminal(id.value), {
  success: (label) => `已打开终端：${label}`,
})
// A remote DSH_HOME is a directory on that machine: what the user can use from
// here is the path and one command, not a file manager that cannot reach it.
const { copy } = useCopy()
const remoteCopyAction = useAction(async (what: 'path' | 'command') => {
  const location = await api.instanceRemoteLocation(id.value)
  await copy(
    what === 'path' ? location.path : location.command,
    what === 'path' ? '远端路径已复制' : '命令已复制，粘到终端里就能连上去',
  )
})

// --- Delete -------------------------------------------------------------------

const deleteAction = useAction(async () => {
  // The record's host decides where it lives: the same instance id read back
  // from another machine is a different record (ADR-0006).
  await api.deleteInstance(id.value, current.value?.host_id)
  return true
})

function confirmDelete() {
  if (!current.value) return
  Modal.confirm({
    title: `删除实例「${name.value}」？`,
    content: `只删除启动器里的这条实例记录：DSH 版本和 DSH_HOME「${homeLabel.value}」都会保留。实例专属图标会一并删除。`,
    okText: `删除`,
    cancelText: `取消`,
    okButtonProps: { status: 'danger' },
    modalClass: 'dl-sheet',
    onOk: async () => {
      const ok = await deleteAction.run()
      if (ok !== true) return
      departing.value = true
      Message.success(`已删除实例「${name.value}」`)
      try {
        await store.refreshInstances()
      } catch {
        // A failed refresh only leaves the board stale; the delete stands.
      }
      void router.push({ name: 'instances' })
    },
  })
}

function addEnvRow() {
  envRows.value.push({ key: '', value: '' })
}
function removeEnvRow(idx: number) {
  envRows.value.splice(idx, 1)
}
</script>

<template>
  <div v-if="loaded" class="dl-page detail-page">
    <header class="dl-head detail-head">
      <div class="dl-head-text">
        <h1 class="detail-title">
          <InstanceAvatar :id="id" :icon="current?.icon" :size="34" />
          <span class="detail-name">{{ name.trim() || `实例设置` }}</span>
          <span
            class="dl-status"
            :class="`is-${status.state}`"
            :title="unreachable ? hostReason : undefined"
          >
            {{ STATUS_LABELS[status.state] }}
          </span>
        </h1>
        <p class="dl-head-sub detail-sub">
          <template v-if="host && !host.builtin">
            <span>{{ host.display_name }}</span>
            <span class="detail-dot">·</span>
          </template>
          <span class="dl-mono" :title="versionTitle">{{ versionLabel }}</span>
          <span class="detail-dot">·</span>
          <span :title="homeTitle">{{ homeLabel }}</span>
        </p>
        <!-- Both addresses of a remote instance, stated apart: where it really
             is, and where it is reachable from here (ADR-0008). The row above
             the board states the same pair; a page opened straight at this
             instance must not have to be read without them. -->
        <p v-if="remoteUrl" class="dl-head-sub detail-sub detail-addresses">
          <span class="detail-dot">{{ `远端位置` }}</span>
          <span class="dl-mono detail-id" :title="remoteUrl">{{
            remoteUrl
          }}</span>
          <template v-if="status.url">
            <span class="detail-dot">·</span>
            <span>{{ `本机可达` }}</span>
            <span class="dl-mono detail-id" :title="status.url">{{
              status.url
            }}</span>
          </template>
        </p>
      </div>
      <div class="dl-head-actions">
        <!-- Unknown is not stopped: the machine that runs this instance did not
             answer, so the one useful move is to ask it again. -->
        <button
          v-if="unreachable"
          class="dl-btn dl-btn--primary"
          :title="hostReason"
          :disabled="retryAction.busy[id]"
          @click="retryAction.run(id)"
        >
          <DlIcon name="refresh" :size="13" />
          {{ `重试` }}
        </button>
        <!-- 打开终端 is the one frequent action; on an unreachable host it steps
             back because it cannot reach the machine. -->
        <button
          class="dl-btn"
          :class="unreachable ? 'dl-btn--quiet' : 'dl-btn--primary'"
          @click="terminalAction.run()"
        >
          <DlIcon name="terminal" :size="13" />
          {{ `打开终端` }}
        </button>
        <a-dropdown trigger="click" position="br">
          <button class="dl-btn dl-btn--quiet">
            {{ `更多` }}
            <DlIcon name="chevron-down" :size="13" />
          </button>
          <template #content>
            <template v-if="remote">
              <a-doption @click="remoteCopyAction.run('path')">
                <DlIcon name="copy" :size="13" />
                {{ `复制路径` }}
              </a-doption>
              <a-doption @click="remoteCopyAction.run('command')">
                <DlIcon name="copy" :size="13" />
                {{ `复制命令` }}
              </a-doption>
            </template>
            <a-doption v-else @click="dirAction.run()">
              <DlIcon name="folder" :size="13" />
              {{ `打开目录` }}
            </a-doption>
            <a-doption @click="logAction.run()">
              <DlIcon name="log" :size="13" />
              {{ `查看日志` }}
            </a-doption>
            <a-doption
              v-if="canManagePlugins(id)"
              @click="openPlugins(id, 'instance-detail')"
            >
              <DlIcon name="plugin" :size="13" />
              {{ `管理插件` }}
            </a-doption>
          </template>
        </a-dropdown>
      </div>
    </header>

    <!-- Two columns: what the instance *is* on the left, the things you attach
         to it on the right. Each row reads like the settings page — label and
         explanation left, control right — so the two forms in this app look
         like one app. -->
    <div class="detail-grid">
      <div class="detail-col">
        <section class="dl-section">
          <div class="dl-section-label">
            {{ `绑定` }}
          </div>
          <div class="dl-surface">
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ `实例名称` }}</span>
              </div>
              <input
                v-model="name"
                class="dl-input detail-ctl"
                :placeholder="`例如：主实例`"
              />
            </div>

            <div v-if="resourceError" class="detail-resource-error">
              <p class="detail-resource-error-title">
                {{ `读不到这台主机的资源` }}
              </p>
              <p class="detail-resource-error-text">
                {{ `这台主机没有回应。请确认它能连接，再点「刷新」重试。` }}
              </p>
              <div class="detail-resource-error-actions">
                <button class="dl-btn dl-btn--quiet" @click="retryHost">
                  {{ `刷新` }}
                </button>
                <details class="detail-error-details">
                  <summary>{{ `查看错误详情` }}</summary>
                  <p class="detail-error-raw">{{ resourceError }}</p>
                </details>
              </div>
            </div>

            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ `DSH 版本` }}</span>
              </div>
              <a-select
                v-model="versionId"
                style="width: 230px"
                :disabled="!!versionReadError"
                :title="versionTitle"
              >
                <a-option v-for="v in hostVersions" :key="v.id" :value="v.id">{{
                  v.version
                }}</a-option>
                <a-option
                  v-if="
                    versionId && !hostVersions.some((v) => v.id === versionId)
                  "
                  :value="versionId"
                  :title="versionTitle"
                  >{{ versionLabel }}</a-option
                >
              </a-select>
            </div>

            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ `DSH_HOME` }}</span>
              </div>
              <a-select
                v-model="homeId"
                style="width: 230px"
                :disabled="!!homeReadError"
                :title="homeTitle"
                @change="onHomePicked"
              >
                <a-option
                  v-for="h in hostHomes"
                  :key="h.id"
                  :value="h.id"
                  :label="h.name"
                  :title="h.path"
                >
                  {{ h.name }} — {{ h.path }}
                </a-option>
                <a-option
                  v-if="homeId && !hostHomes.some((h) => h.id === homeId)"
                  :value="homeId"
                  :label="homeLabel"
                  :title="homeTitle"
                >
                  {{ homeRecord ? `${homeLabel} — ${homeTitle}` : homeLabel }}
                </a-option>
              </a-select>
            </div>

            <div class="dl-row">
              <span class="dl-row-label">{{ `默认 Profile` }}</span>
              <a-select
                v-model="defaultProfile"
                style="width: 230px"
                :placeholder="`启动时默认选择的 Profile`"
                :disabled="!!profilesError"
                allow-clear
              >
                <a-option v-for="p in profiles" :key="p" :value="p">{{
                  p
                }}</a-option>
                <a-option
                  v-if="defaultProfile && !profiles.includes(defaultProfile)"
                  :value="defaultProfile"
                  >{{ defaultProfile }}</a-option
                >
              </a-select>
            </div>
          </div>
        </section>

        <section class="dl-section">
          <div class="dl-section-label">
            {{ `启动` }}
          </div>
          <div class="dl-surface">
            <div class="dl-row">
              <div class="dl-row-info">
                <span class="dl-row-label">{{ `端口号` }}</span>
                <span class="dl-row-desc">{{ `留空使用随机端口。` }}</span>
              </div>
              <div class="detail-inline">
                <input
                  v-model="portInput"
                  class="dl-input dl-input--mono detail-port"
                  :class="{ 'dl-input--error': !!portError }"
                  :placeholder="`留空 = 随机端口`"
                />
                <span v-if="portError" class="dl-error-text">{{
                  portError
                }}</span>
              </div>
            </div>
          </div>
        </section>
      </div>

      <div class="detail-col">
        <section class="dl-section">
          <div class="dl-section-label">{{ `实例图标` }}</div>
          <div class="dl-surface detail-pad">
            <div class="icon-block">
              <span class="icon-preview">
                <img v-if="iconUrl" :src="iconUrl" alt="" />
                <InstanceAvatar
                  v-else
                  :id="id"
                  :icon="null"
                  :size="46"
                  class="icon-preview-fallback"
                />
              </span>
              <div class="icon-controls">
                <div class="detail-inline">
                  <input
                    v-model="iconInput"
                    class="dl-input icon-url"
                    :placeholder="`https:// 图片地址`"
                  />
                  <button
                    class="dl-btn dl-btn--quiet"
                    :disabled="!iconInput.trim() || iconAction.busy['*']"
                    @click="applyIconInput"
                  >
                    {{ `应用` }}
                  </button>
                </div>
                <div class="detail-inline">
                  <button
                    class="dl-btn dl-btn--quiet"
                    :disabled="iconAction.busy['*']"
                    @click="pickIconFile"
                  >
                    {{ `选择图片` }}
                  </button>
                  <button
                    v-if="iconUrl"
                    class="dl-btn dl-btn--danger"
                    @click="clearIcon"
                  >
                    {{ `恢复默认` }}
                  </button>
                </div>
              </div>
            </div>
          </div>
        </section>

        <section class="dl-section">
          <div class="dl-section-label">
            {{ `环境变量覆写` }}
            <span class="dl-section-count">{{ envRows.length }}</span>
          </div>
          <div class="dl-surface detail-pad">
            <div v-for="(row, idx) in envRows" :key="idx" class="env-row">
              <input
                v-model="row.key"
                class="dl-input dl-input--mono env-key"
                :class="{ 'dl-input--error': !!envKeyError(row) }"
                :placeholder="`变量名`"
                :title="row.key"
              />
              <span class="env-eq">=</span>
              <input
                v-model="row.value"
                class="dl-input dl-input--mono env-value"
                :placeholder="`值`"
              />
              <button class="dl-icon-btn" @click="removeEnvRow(idx)">
                <DlIcon name="trash" :size="14" />
              </button>
              <span v-if="envKeyError(row)" class="dl-error-text env-error">{{
                envKeyError(row)
              }}</span>
            </div>
            <button class="dl-btn dl-btn--quiet" @click="addEnvRow">
              <DlIcon name="plus" :size="13" />
              {{ `添加一行` }}
            </button>
          </div>
        </section>

        <section class="dl-section">
          <div class="dl-section-label">
            {{ `危险操作` }}
          </div>
          <div class="dl-surface detail-pad danger-card">
            <div class="danger-text">
              <span class="danger-title">{{ `删除这个实例` }}</span>
            </div>
            <button
              class="dl-btn dl-btn--danger"
              :disabled="deleteAction.busy['*']"
              @click="confirmDelete"
            >
              <DlIcon name="trash" :size="13" />
              {{ `删除` }}
            </button>
          </div>
        </section>
      </div>
    </div>

    <!-- Footer: the sticky save bar for the whole form. -->
    <div class="detail-footer">
      <span v-if="dirty" class="footer-dirty">{{ `有未保存的修改` }}</span>
      <button
        class="dl-btn dl-btn--quiet"
        :disabled="!dirty || saving"
        @click="restoreChanges"
      >
        {{ `还原修改` }}
      </button>
      <button
        class="dl-btn dl-btn--primary"
        :disabled="!formValid || !dirty || saving"
        @click="onSave"
      >
        {{ `保存` }}
      </button>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.detail-page {
  padding-bottom: 96px;
}

.detail-title {
  display: flex;
  align-items: center;
  gap: 10px;
}

.detail-name {
  font-size: var(--dl-text-2xl);
  font-weight: 600;
}

.detail-sub {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 3px;
}

.detail-dot {
  color: var(--dl-ink-4);
}

.detail-id {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 220px;
  color: var(--dl-ink-4);
}

// A resource this page needs on the host could not be read. The saved values
// stay on screen; this block states the reason once and keeps the raw reply
// behind 查看错误详情.
.detail-resource-error {
  padding: 12px 16px 14px;
  border-top: 1px solid var(--dl-line);
}

.detail-resource-error-title {
  margin: 0;
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.detail-resource-error-text {
  margin: 4px 0 0;
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
}

.detail-resource-error-actions {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  margin-top: 10px;
}

.detail-error-details {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);

  summary {
    cursor: pointer;
    color: var(--dl-ink-2);
  }
}

.detail-error-raw {
  margin: 6px 0 0;
  font-family: var(--dl-font-mono);
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-danger);
  overflow-wrap: anywhere;
}

// Left: what the instance is. Right: what is attached to it.
.detail-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr);
  gap: 0 28px;
  align-items: start;
}

// Controls keep a readable width instead of stretching across the card.
// Selects carry the width inline: Arco's root element never receives Vue's
// scope id, so a scoped rule can never match it.
.detail-ctl {
  width: 230px;
  flex-shrink: 0;
}

.detail-port {
  width: 120px;
}

.detail-pad {
  padding: 16px;
}

.detail-inline {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.icon-block {
  display: flex;
  align-items: center;
  gap: 14px;
}

.icon-preview {
  display: inline-flex;
  width: 46px;
  height: 46px;
  flex-shrink: 0;
  overflow: hidden;

  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: 10px;
  }

  .icon-preview-fallback {
    width: 46px;
    height: 46px;
  }
}

.icon-controls {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.icon-url {
  flex: 1 1 150px;
  min-width: 130px;
}

.env-desc {
  margin: 0 0 12px;
}

.env-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;

  .env-key {
    width: 150px;
  }

  .env-eq {
    color: var(--dl-ink-4);
    font-family: var(--dl-font-mono);
  }

  .env-value {
    flex: 1;
    min-width: 0;
  }
}

.env-error {
  flex-basis: 100%;
}

.danger-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  border-color: var(--dl-danger-soft);

  .danger-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .danger-title {
    font-size: var(--dl-text-md);
    font-weight: 600;
    color: var(--dl-ink);
  }
}

.detail-footer {
  position: sticky;
  bottom: 0;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 28px;
  padding: 14px 0;
  border-top: 1px solid var(--dl-line);
  background: linear-gradient(180deg, transparent, var(--dl-canvas) 30%);
}

.footer-dirty {
  margin-right: auto;
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

// One column once the two side by side stop fitting.
@media (max-width: 1040px) {
  .detail-grid {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>

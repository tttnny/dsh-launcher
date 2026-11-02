<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { LauncherUpdateInfo, ThemeMode } from '@/api/types'
import { useLauncherStore } from '@/stores/launcher'
import { useCopy } from '@/composables/useCopy'
import PageHeader from '@/components/PageHeader.vue'
import DlIcon from '@/components/DlIcon.vue'

// 设置页只做一件事：把启动器的全局选项摆在一列里，改一下就保存。
// 没有侧边目录，没有底部保存栏；每处改动都在失焦或切换时写回后端。

const store = useLauncherStore()
const { copy } = useCopy()

// --- 读取失败与重试 ---------------------------------------------------------

const reloadError = ref('')
const settingsError = computed(
  () =>
    reloadError.value ||
    (store.initErrors.find((e) => e.startsWith('读取设置失败')) ?? ''),
)

async function reloadSettings() {
  try {
    store.settings = await api.getSettings()
    store.initErrors = store.initErrors.filter(
      (e) => !e.startsWith('读取设置失败'),
    )
    reloadError.value = ''
    Message.success('已重新读取设置')
  } catch (e) {
    reloadError.value = `读取设置失败：${String(e)}`
    Message.error(String(e))
  }
}

// --- 改动即保存 -------------------------------------------------------------
//
// 所有控件直接绑定 store.settings。改动后短暂合并，再整份写回；成功给一次
// 「设置已保存」，失败用后端原文提示并把界面拉回后端当前的设置。

let saveTimer: ReturnType<typeof setTimeout> | undefined

function persist() {
  if (saveTimer) clearTimeout(saveTimer)
  saveTimer = setTimeout(() => {
    saveTimer = undefined
    void flushSave()
  }, 420)
}

async function flushSave() {
  const snapshot = { ...store.settings }
  try {
    const saved = await api.updateSettings(snapshot)
    // 保存期间若又改了，保留界面上的新值，等下一次写回，不让旧回包覆盖。
    if (JSON.stringify({ ...store.settings }) === JSON.stringify(snapshot)) {
      store.settings = saved
    }
    Message.success('设置已保存')
  } catch (e) {
    Message.error(String(e))
    try {
      store.settings = await api.getSettings()
    } catch {
      // 读取也失败时保留界面值，让用户看到刚才的改动。
    }
  }
}

// --- 外观 -------------------------------------------------------------------

function setTheme(mode: ThemeMode) {
  if (store.settings.theme === mode) return
  store.settings.theme = mode
  persist()
}

const THEME_OPTIONS: { value: ThemeMode; label: string }[] = [
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
  { value: 'system', label: '跟随系统' },
]

// --- 日志等级 ---------------------------------------------------------------

const LOG_LEVELS = [
  { value: 'debug', label: '调试' },
  { value: 'info', label: '信息' },
  { value: 'warn', label: '警告' },
  { value: 'error', label: '错误' },
]

// --- 网络代理 ---------------------------------------------------------------

function onPortChange(e: Event) {
  const input = e.target as HTMLInputElement
  const parsed = Number(input.value)
  if (!Number.isFinite(parsed) || parsed < 1) {
    input.value = String(store.settings.proxy_port)
    return
  }
  store.settings.proxy_port = Math.min(65535, Math.round(parsed))
  input.value = String(store.settings.proxy_port)
  persist()
}

// --- 启动器更新 -------------------------------------------------------------

const launcherVersion = ref('')
const versionLoading = ref(true)
const versionError = ref('')
const checking = ref(false)
const updateInfo = ref<LauncherUpdateInfo | null>(null)

// 渠道由当前安装的版本决定：带 -dev 的构建跟预览版，其余跟正式版。
const updateChannel = computed<'dev' | 'release'>(() =>
  launcherVersion.value.includes('-dev') ? 'dev' : 'release',
)
const channelLabel = computed(() =>
  updateChannel.value === 'dev' ? '预览版' : '正式版',
)

async function loadVersion() {
  versionLoading.value = true
  versionError.value = ''
  try {
    launcherVersion.value = await api.getLauncherVersion()
  } catch (e) {
    versionError.value = String(e)
  } finally {
    versionLoading.value = false
  }
}

async function checkUpdate() {
  if (checking.value) return
  checking.value = true
  try {
    const info = await api.checkLauncherUpdate(updateChannel.value)
    updateInfo.value = info
    if (info.up_to_date) Message.success('当前已是最新版本')
    else Message.info(`发现新版本 v${info.latest ?? ''}`)
  } catch (e) {
    Message.error(String(e))
  } finally {
    checking.value = false
  }
}

async function openUpdate() {
  const url = updateInfo.value?.url
  if (!url) return
  try {
    await api.openExternal(url)
  } catch (e) {
    Message.error(String(e))
  }
}

// --- 数据 -------------------------------------------------------------------

const launcherDir = ref('')
const dirLoading = ref(true)
const dirError = ref('')

async function loadLauncherDir() {
  dirLoading.value = true
  dirError.value = ''
  try {
    launcherDir.value = await api.getLauncherDirectory()
  } catch (e) {
    dirError.value = String(e)
  } finally {
    dirLoading.value = false
  }
}

async function copyDir() {
  if (!launcherDir.value) return
  await copy(launcherDir.value, '已复制数据目录路径')
}

async function openDir() {
  try {
    await api.openLauncherDirectory()
    Message.success('已打开数据目录')
  } catch (e) {
    Message.error(String(e))
  }
}

async function openLog() {
  try {
    await api.openLauncherLog()
    Message.success('已打开运行日志')
  } catch (e) {
    Message.error(String(e))
  }
}

onMounted(() => {
  void loadVersion()
  void loadLauncherDir()
})

onUnmounted(() => {
  // 离开设置页时把还没写回的改动立即写回：定时器不再在组件卸载后触发，
  // 刚改的选项也不会丢。
  if (!saveTimer) return
  clearTimeout(saveTimer)
  saveTimer = undefined
  void flushSave()
})
</script>

<template>
  <div class="page">
    <PageHeader
      title="设置"
      description="调整启动器的外观、启动行为、网络和更新。"
    />

    <div v-if="!store.loaded" class="surface surface--pad">
      <div class="inline muted"><span class="spinner" /> 正在读取设置…</div>
    </div>

    <div v-else-if="settingsError" class="notice notice--danger">
      <div class="notice-body">
        <span class="notice-title">设置读取失败</span>
        <div class="mono">{{ settingsError }}</div>
      </div>
      <button class="btn btn--quiet btn--sm" @click="reloadSettings">
        重试
      </button>
    </div>

    <template v-else>
      <!-- 外观 -->
      <section class="section">
        <div class="section-head">
          <span class="section-title">外观</span>
          <span class="section-rule" />
        </div>
        <div class="surface">
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">主题</span>
              <span class="field-hint">
                跟随系统时，随 macOS 的外观自动切换深浅色。
              </span>
            </div>
            <div class="seg" role="group" aria-label="主题">
              <button
                v-for="opt in THEME_OPTIONS"
                :key="opt.value"
                type="button"
                class="seg-item"
                :class="{ 'is-active': store.settings.theme === opt.value }"
                @click="setTheme(opt.value)"
              >
                {{ opt.label }}
              </button>
            </div>
          </div>
        </div>
      </section>

      <!-- 窗口与启动 -->
      <section class="section">
        <div class="section-head">
          <span class="section-title">窗口与启动</span>
          <span class="section-rule" />
        </div>
        <div class="surface">
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">关闭主窗口时最小化到系统托盘</span>
              <span class="field-hint">
                关闭窗口后启动器仍在托盘运行，已启动的实例不受影响。
              </span>
            </div>
            <a-switch
              v-model="store.settings.minimize_to_tray"
              @change="persist"
            />
          </div>
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">开机自动启动</span>
              <span class="field-hint">登录系统后自动打开启动器。</span>
            </div>
            <a-switch v-model="store.settings.autostart" @change="persist" />
          </div>
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">外部终端</span>
              <span class="field-hint">
                在实例上打开终端时使用的终端程序。
              </span>
            </div>
            <a-select
              v-model="store.settings.terminal"
              style="width: 200px"
              @change="persist"
            >
              <a-option value="system">系统终端（Terminal）</a-option>
              <a-option value="ghostty">Ghostty</a-option>
            </a-select>
          </div>
        </div>
      </section>

      <!-- 日志 -->
      <section class="section">
        <div class="section-head">
          <span class="section-title">日志</span>
          <span class="section-rule" />
        </div>
        <div class="surface">
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">日志等级</span>
              <span class="field-hint">
                启动器写入运行日志的详细程度，等级越高记录越少。
              </span>
            </div>
            <a-select
              v-model="store.settings.log_level"
              style="width: 160px"
              @change="persist"
            >
              <a-option
                v-for="lv in LOG_LEVELS"
                :key="lv.value"
                :value="lv.value"
              >
                {{ lv.label }}
              </a-option>
            </a-select>
          </div>
        </div>
      </section>

      <!-- 网络代理 -->
      <section class="section">
        <div class="section-head">
          <span class="section-title">网络代理</span>
          <span class="section-rule" />
        </div>
        <div class="surface">
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">启用代理</span>
              <span class="field-hint">
                启动器访问网络时走下面的 HTTP 代理。
              </span>
            </div>
            <a-switch
              v-model="store.settings.proxy_enabled"
              @change="persist"
            />
          </div>

          <div class="proxy-body">
            <div class="proxy-grid">
              <label class="proxy-field proxy-field--grow">
                <span class="field-label">代理地址</span>
                <input
                  v-model="store.settings.proxy_url"
                  class="input input--mono"
                  type="text"
                  spellcheck="false"
                  placeholder="http://127.0.0.1"
                  :disabled="!store.settings.proxy_enabled"
                  @change="persist"
                />
                <span class="field-hint">只填协议和主机，不含端口。</span>
              </label>
              <label class="proxy-field proxy-field--port">
                <span class="field-label">端口</span>
                <input
                  class="input input--mono"
                  type="number"
                  min="1"
                  max="65535"
                  inputmode="numeric"
                  :value="store.settings.proxy_port"
                  :disabled="!store.settings.proxy_enabled"
                  @change="onPortChange"
                />
              </label>
            </div>
            <label class="proxy-field">
              <span class="field-label">绕过列表</span>
              <input
                v-model="store.settings.no_proxy"
                class="input input--mono"
                type="text"
                spellcheck="false"
                placeholder="127.0.0.1,localhost,::1"
                :disabled="!store.settings.proxy_enabled"
                @change="persist"
              />
              <span class="field-hint">用逗号分隔，这些地址不走代理。</span>
            </label>
          </div>

          <div class="field-row field-row--after-body">
            <div class="field-row-info">
              <span class="field-label">在 DSH 中生效</span>
              <span class="field-hint">
                启动 DSH 实例时也注入同一份代理设置，下次启动生效。
              </span>
            </div>
            <a-switch
              v-model="store.settings.proxy_apply_dsh"
              :disabled="!store.settings.proxy_enabled"
              @change="persist"
            />
          </div>
        </div>
      </section>

      <!-- 启动器更新 -->
      <section class="section">
        <div class="section-head">
          <span class="section-title">启动器更新</span>
          <span class="section-rule" />
        </div>
        <div class="surface">
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">当前版本</span>
            </div>
            <span v-if="versionLoading" class="muted">读取中…</span>
            <span v-else-if="versionError" class="error-text"> 读取失败 </span>
            <span v-else class="mono version-value">
              v{{ launcherVersion }}
            </span>
          </div>
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">更新渠道</span>
              <span class="field-hint">由当前安装的版本决定。</span>
            </div>
            <span
              class="tag"
              :class="updateChannel === 'dev' ? 'tag--warn' : 'tag--muted'"
            >
              {{ channelLabel }}
            </span>
          </div>
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">检查更新</span>
              <span class="field-hint">
                从 GitHub 检查 {{ channelLabel }}渠道是否发布了新版本。
              </span>
            </div>
            <button
              class="btn btn--quiet"
              :disabled="checking"
              @click="checkUpdate"
            >
              <span v-if="checking" class="spinner" />
              {{ checking ? '检查中…' : '检查更新' }}
            </button>
          </div>
          <div
            v-if="updateInfo"
            class="update-result"
            :class="{ 'is-ok': updateInfo.up_to_date }"
          >
            <template v-if="updateInfo.up_to_date"> 当前已是最新版本 </template>
            <template v-else>
              <span>
                发现新版本
                <span class="mono">v{{ updateInfo.latest ?? '' }}</span>
              </span>
              <button
                v-if="updateInfo.url"
                class="btn btn--primary btn--sm"
                @click="openUpdate"
              >
                <DlIcon name="download" :size="14" />
                下载
              </button>
            </template>
          </div>
        </div>
      </section>

      <!-- 数据 -->
      <section class="section">
        <div class="section-head">
          <span class="section-title">数据</span>
          <span class="section-rule" />
        </div>
        <div class="surface">
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">启动器数据目录</span>
              <span class="field-hint">
                启动器的日志和本地状态保存在这个目录。
              </span>
            </div>
            <div class="data-actions">
              <span v-if="dirLoading" class="muted">读取中…</span>
              <span v-else-if="dirError" class="error-text">读取失败</span>
              <span v-else class="mono path-text" :title="launcherDir">
                {{ launcherDir }}
              </span>
              <button
                class="icon-btn"
                title="复制路径"
                :disabled="!launcherDir"
                @click="copyDir"
              >
                <DlIcon name="copy" :size="15" />
              </button>
              <button class="btn btn--quiet btn--sm" @click="openDir">
                <DlIcon name="folder" :size="14" />
                打开目录
              </button>
            </div>
          </div>
          <div class="field-row">
            <div class="field-row-info">
              <span class="field-label">运行日志</span>
              <span class="field-hint">查看启动器最近的运行日志。</span>
            </div>
            <button class="btn btn--quiet btn--sm" @click="openLog">
              <DlIcon name="log" :size="14" />
              查看运行日志
            </button>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>

<style lang="scss" scoped>
// 分段选择：外观主题用三选一，比下拉更直接。
.seg {
  display: inline-flex;
  gap: 2px;
  padding: 3px;
  border: none;
  border-radius: 8px;
  background: var(--dl-surface-3);
}

.seg-item {
  height: 26px;
  padding: 0 12px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--dl-ink-3);
  font-family: inherit;
  font-size: var(--dl-text-sm);
  font-weight: 550;
  cursor: pointer;
  transition:
    background-color var(--dl-fast) var(--dl-ease),
    color var(--dl-fast) var(--dl-ease);

  &:hover {
    color: var(--dl-ink);
  }

  &.is-active {
    background: var(--dl-surface);
    color: var(--dl-ink);
    box-shadow: var(--dl-lift-1);
  }
}

// 代理的输入区夹在两个开关行之间，自己补上分隔线。
.proxy-body {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 14px 16px;
  border-top: 1px solid var(--dl-line);
}

.field-row--after-body {
  border-top: 1px solid var(--dl-line);
}

.proxy-grid {
  display: flex;
  gap: 14px;
}

.proxy-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.proxy-field--grow {
  flex: 1;
}

.proxy-field--port {
  width: 132px;
  flex: none;
}

.version-value {
  color: var(--dl-ink-2);
  font-size: var(--dl-text-md);
}

.update-result {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 16px;
  border-top: 1px solid var(--dl-line);
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);

  &.is-ok {
    color: var(--dl-run);
  }
}

.data-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.path-text {
  max-width: 320px;
  overflow: hidden;
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.shortcut-row {
  min-height: 46px;
}

.shortcut-label {
  color: var(--dl-ink);
  font-size: var(--dl-text-md);
}

.shortcut-keys {
  display: flex;
  align-items: center;
  gap: 5px;
  flex-shrink: 0;
}

.kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: 22px;
  padding: 0 6px;
  border: 1px solid var(--dl-line-strong);
  border-bottom-width: 2px;
  border-radius: 5px;
  background: var(--dl-surface-2);
  color: var(--dl-ink-2);
  font-family: var(--dl-font-mono);
  font-size: 11px;
}

.kbd-plus {
  color: var(--dl-ink-4);
  font-size: 11px;
}
</style>

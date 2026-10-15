<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { ToolSource } from '@/api/types'
import { useCopy } from '@/composables/useCopy'
import { useLauncherStore } from '@/stores/launcher'
import DlIcon from '@/components/DlIcon.vue'

// The toolchain check. It states the same version floors the backend resolver
// enforces and offers the install command the backend picked for this machine,
// so the page never invents a number or a package manager.
//
// The verdict is per source: whether the tool comes from the user's login-shell
// PATH (the one a terminal would find), from a directory the launcher searched
// on its own, or from a path the user picked. Anything but `system` means the
// terminal does not see it, so those tools also get the picker.

type ToolId = 'node' | 'pnpm'

const TOOL_IDS: ToolId[] = ['node', 'pnpm']
const TOOL_LABELS: Record<ToolId, string> = { node: 'Node.js', pnpm: 'pnpm' }

const router = useRouter()
const { t } = useI18n()
const { copy } = useCopy()
const store = useLauncherStore()

const checking = ref(false)
const picking = ref<ToolId | null>(null)

function statusOf(id: ToolId) {
  return (id === 'node' ? store.runtime?.node : store.runtime?.pnpm) ?? null
}

interface ToolRow {
  id: ToolId
  name: string
  ok: boolean
  version: string | null
  path: string | null
  source: ToolSource | null
}

const tools = computed<ToolRow[]>(() =>
  TOOL_IDS.map((id) => {
    const status = statusOf(id)
    return {
      id,
      name: TOOL_LABELS[id],
      ok: status?.installed ?? false,
      version: status?.version ?? null,
      path: status?.path ?? null,
      source: status?.source ?? null,
    }
  }),
)

const nodeOk = computed(() => statusOf('node')?.installed ?? false)
const pnpmOk = computed(() => statusOf('pnpm')?.installed ?? false)
// Node is what DSH needs to boot; pnpm is needed to install a version and to
// manage plugins, and the launcher no longer installs one itself.
const allOk = computed(() => nodeOk.value && pnpmOk.value)
const belowRecommended = computed(() => store.runtime?.node_below_recommended ?? false)

const requirements = computed(() => store.runtime?.requirements)
const nodeCommand = computed(() => requirements.value?.node_install_command ?? '')
const pnpmCommand = computed(() => requirements.value?.pnpm_install_command ?? '')

const overridePath = (id: ToolId) => statusOf(id)?.manual_path ?? null
// A pick that no longer resolves: the backend falls back to its own detection
// and still reports the chosen path, so the page can say so and let the user
// replace or clear it.
const overrideBroken = (id: ToolId) => {
  const status = statusOf(id)
  return !!status?.manual_path && status.source !== 'manual'
}

// The tools the picker is worth showing for: a terminal would not find them
// (missing, or found by the launcher on its own), or the user already picked a
// path and must be able to see, change and clear it — including a pick that has
// gone stale while the tool itself is back in the system PATH.
const manualTools = computed(() =>
  tools.value.filter((tool) => !tool.ok || tool.source !== 'system' || !!overridePath(tool.id)),
)

function sourceLabel(tool: ToolRow): string {
  if (!tool.ok) return t('setup.sourceMissing')
  if (tool.source === 'system') return t('setup.sourceSystem')
  if (tool.source === 'fallback') return t('setup.sourceFallback')
  if (tool.source === 'manual') return t('setup.sourceManual')
  return ''
}

const copied = ref('')

async function copyCommand(text: string, key: string) {
  if (!(await copy(text))) return
  copied.value = key
  window.setTimeout(() => {
    if (copied.value === key) copied.value = ''
  }, 2000)
}

/** Re-reads the system environment variables: the user may have just fixed their PATH. */
async function recheck() {
  checking.value = true
  try {
    await store.checkRuntime(true)
    if (nodeOk.value && pnpmOk.value) Message.success(t('setup.allReady'))
  } finally {
    checking.value = false
  }
}

// A toolchain installed in a terminal becomes visible as soon as the user comes
// back to the window, so the page does not make them hunt for a button.
async function onFocus() {
  if (checking.value) return
  await store.checkRuntime()
}

/** Points one tool at the file the user picked; `null` goes back to automatic detection. */
async function applyOverride(id: ToolId, path: string | null) {
  try {
    store.runtime = await api.setToolchainOverride(id, path)
    Message.success(
      t(path ? 'setup.overrideApplied' : 'setup.overrideCleared', { tool: TOOL_LABELS[id] }),
    )
  } catch (e) {
    Message.error(String(e))
  }
}

async function pickFile(id: ToolId) {
  if (!api.isTauri) {
    Message.info(t('setup.browserPickHint'))
    return
  }
  picking.value = id
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    // No filters: node and pnpm carry no file extension, so a filter would hide them.
    const picked = await open({ multiple: false })
    if (typeof picked === 'string') await applyOverride(id, picked)
  } catch (e) {
    Message.error(String(e))
  } finally {
    picking.value = null
  }
}

function clearOverride(id: ToolId) {
  return applyOverride(id, null)
}

onMounted(() => window.addEventListener('focus', onFocus))
onUnmounted(() => window.removeEventListener('focus', onFocus))
</script>

<template>
  <div class="dl-page setup-page">
    <header class="dl-head">
      <div class="dl-head-text">
        <h1>{{ t('setup.title') }}</h1>
        <p class="dl-head-sub">{{ t('setup.desc') }}</p>
      </div>
      <div class="dl-head-actions">
        <button class="dl-btn dl-btn--quiet" :disabled="checking" @click="recheck">
          <DlIcon name="refresh" :size="13" />
          {{ t('setup.recheck') }}
        </button>
        <button v-if="allOk" class="dl-btn dl-btn--primary" @click="router.push({ name: 'instances' })">
          {{ t('setup.enterApp') }}
        </button>
      </div>
    </header>

    <!-- Status first: the two tools, resolved paths and what to do if a leg is
         missing. One block per missing tool, so a machine lacking both is told
         both things at once. -->
    <section class="dl-section">
      <div class="dl-section-label">
        {{ t('settings.environment.title') }}
        <span class="dl-section-count">
          <span class="dl-tag" :class="allOk ? 'dl-tag--run' : 'dl-tag--danger'">
            {{ allOk ? t('setup.allReady') : t('nav.envMissing') }}
          </span>
        </span>
      </div>

      <div class="dl-surface">
        <div v-for="tool in tools" :key="tool.name" class="dl-row">
          <span class="tool-mark" :class="tool.ok ? 'is-on' : 'is-off'">
            <DlIcon :name="tool.ok ? 'check' : 'alert'" :size="13" />
          </span>
          <div class="tool-info">
            <span class="tool-name">{{ tool.name }}</span>
            <span v-if="tool.ok && tool.path" class="tool-path dl-mono">{{ tool.path }}</span>
            <span class="tool-source">{{ sourceLabel(tool) }}</span>
          </div>
          <span class="dl-chip">{{ tool.ok ? tool.version : t('setup.missing') }}</span>
        </div>
      </div>

      <p v-if="nodeOk && belowRecommended" class="setup-note">
        <DlIcon name="alert" :size="13" />
        {{ t('setup.nodeBelowRecommended', { v: requirements?.recommended_node_major }) }}
      </p>
    </section>

    <!-- The two ways out of a missing or non-system toolchain: point the
         launcher at the file, or fix PATH in a terminal and re-check. Shown for
         every tool the terminal would not find, plus any tool the user already
         picked, so their choice stays visible and revisable. -->
    <section v-if="manualTools.length" class="dl-section">
      <div class="dl-section-label">{{ t('setup.manualSection') }}</div>

      <div v-for="tool in manualTools" :key="tool.id" class="dl-surface pick">
        <div class="pick-head">
          <span class="pick-title">{{ t('setup.manualToolTitle', { tool: tool.name }) }}</span>
          <span class="pick-desc">{{ t('setup.manualDesc', { tool: tool.name }) }}</span>
          <span v-if="overridePath(tool.id)" class="pick-path dl-mono">
            {{ overridePath(tool.id) }}
          </span>
          <span v-if="overrideBroken(tool.id)" class="pick-broken">
            <DlIcon name="alert" :size="13" />
            {{ t('setup.manualBroken') }}
          </span>
        </div>
        <div class="pick-actions">
          <button class="dl-btn dl-btn--quiet" :disabled="picking === tool.id" @click="pickFile(tool.id)">
            <DlIcon name="folder" :size="13" />
            {{ t('setup.pickFile') }}
          </button>
          <button
            v-if="overridePath(tool.id)"
            class="dl-btn dl-btn--quiet"
            :disabled="picking === tool.id"
            @click="clearOverride(tool.id)"
          >
            {{ t('setup.clearOverride') }}
          </button>
        </div>
      </div>
    </section>

    <section v-if="!allOk" class="dl-section">
      <div class="dl-section-label">{{ t('setup.howTo') }}</div>

      <div v-if="!nodeOk" class="dl-surface guide">
        <div class="guide-head">
          <span class="guide-title">{{ t('setup.installNode') }}</span>
          <span class="guide-desc">{{ t('setup.nodeCommandDesc') }}</span>
        </div>
        <div class="cmd-row">
          <code class="cmd-text">{{ nodeCommand }}</code>
          <button class="dl-btn dl-btn--quiet" @click="copyCommand(nodeCommand, 'node')">
            <DlIcon name="copy" :size="13" />
            {{ copied === 'node' ? t('common.copied') : t('common.copy') }}
          </button>
        </div>
      </div>

      <div v-if="!pnpmOk" class="dl-surface guide">
        <div class="guide-head">
          <span class="guide-title">{{ t('setup.installPnpm') }}</span>
          <span class="guide-desc">
            {{ t('setup.pnpmCommandDesc', { major: requirements?.min_pnpm_major }) }}
          </span>
        </div>
        <div class="cmd-row">
          <code class="cmd-text">{{ pnpmCommand }}</code>
          <button class="dl-btn dl-btn--quiet" @click="copyCommand(pnpmCommand, 'pnpm')">
            <DlIcon name="copy" :size="13" />
            {{ copied === 'pnpm' ? t('common.copied') : t('common.copy') }}
          </button>
        </div>
      </div>

      <p class="setup-note">{{ t('setup.afterInstallHint') }}</p>
    </section>
  </div>
</template>

<style lang="scss" scoped>
.setup-page {
  max-width: 720px;
}

.tool-mark {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  flex-shrink: 0;
  border-radius: var(--dl-r-ctl);

  &.is-on {
    background: var(--dl-run-soft);
    color: var(--dl-run);
  }

  &.is-off {
    background: var(--dl-danger-soft);
    color: var(--dl-danger);
  }
}

.tool-info {
  display: flex;
  flex-direction: column;
  gap: 1px;
  flex: 1;
  min-width: 0;
}

.tool-name {
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.tool-path {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tool-source {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
}

.setup-note {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 10px 2px 0;
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-warn);
}

.guide {
  padding: 16px;

  & + .guide {
    margin-top: 12px;
  }
}

.pick {
  padding: 16px;

  & + .pick {
    margin-top: 12px;
  }
}

.pick-head {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-bottom: 12px;
}

.pick-title {
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.pick-desc {
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
}

.pick-path {
  font-size: var(--dl-text-sm);
  color: var(--dl-ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pick-broken {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-warn);
}

.pick-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.guide-head {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-bottom: 12px;
}

.guide-title {
  font-size: var(--dl-text-md);
  font-weight: 600;
  color: var(--dl-ink);
}

.guide-desc {
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
}

.cmd-row {
  display: flex;
  align-items: stretch;
  gap: 8px;
}

.cmd-text {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  padding: 10px 12px;
  border-radius: var(--dl-r-ctl);
  background: var(--dl-surface-3);
  color: var(--dl-ink);
  font-family: var(--dl-font-mono);
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-all;
  user-select: text;
}
</style>

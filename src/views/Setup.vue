<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useCopy } from '@/composables/useCopy'
import { useLauncherStore } from '@/stores/launcher'
import DlIcon from '@/components/DlIcon.vue'

// The toolchain check. It states the same version floors the backend resolver
// enforces and offers the install command the backend picked for this machine,
// so the page never invents a number or a package manager.

const router = useRouter()
const { t } = useI18n()
const { copy } = useCopy()
const store = useLauncherStore()

const checking = ref(false)

const node = computed(() => store.runtime?.node)
const pnpm = computed(() => store.runtime?.pnpm)
const nodeOk = computed(() => node.value?.installed ?? false)
const pnpmOk = computed(() => pnpm.value?.installed ?? false)
// Node is what DSH needs to boot; pnpm is needed to install a version and to
// manage plugins, and the launcher no longer installs one itself.
const allOk = computed(() => nodeOk.value && pnpmOk.value)
const belowRecommended = computed(() => store.runtime?.node_below_recommended ?? false)

const requirements = computed(() => store.runtime?.requirements)
const nodeCommand = computed(() => requirements.value?.node_install_command ?? '')
const pnpmCommand = computed(() => requirements.value?.pnpm_install_command ?? '')

const copied = ref('')

async function copyCommand(text: string, key: string) {
  if (!(await copy(text))) return
  copied.value = key
  window.setTimeout(() => {
    if (copied.value === key) copied.value = ''
  }, 2000)
}

async function recheck() {
  checking.value = true
  try {
    await store.checkRuntime()
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

onMounted(() => window.addEventListener('focus', onFocus))
onUnmounted(() => window.removeEventListener('focus', onFocus))

interface ToolRow {
  name: string
  ok: boolean
  version?: string | null
  path?: string | null
}

const tools = computed<ToolRow[]>(() => [
  { name: 'Node.js', ok: nodeOk.value, version: node.value?.version, path: node.value?.path },
  { name: 'pnpm', ok: pnpmOk.value, version: pnpm.value?.version, path: pnpm.value?.path },
])
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
          </div>
          <span class="dl-chip">{{ tool.ok ? tool.version : t('setup.missing') }}</span>
        </div>
      </div>

      <p v-if="nodeOk && belowRecommended" class="setup-note">
        <DlIcon name="alert" :size="13" />
        {{ t('setup.nodeBelowRecommended', { v: requirements?.recommended_node_major }) }}
      </p>
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

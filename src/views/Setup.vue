<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useCopy } from '@/composables/useCopy'
import { useLauncherStore } from '@/stores/launcher'

const router = useRouter()
const { t } = useI18n()
const { copy } = useCopy()
const store = useLauncherStore()

const checking = ref(false)

const node = computed(() => store.runtime?.node)
const pnpm = computed(() => store.runtime?.pnpm)
const nodeOk = computed(() => node.value?.installed ?? false)
const pnpmOk = computed(() => pnpm.value?.installed ?? false)
// Readiness is about usability: node is what DSH needs to boot; pnpm is needed
// to install a version or manage plugins, and the launcher no longer installs
// one itself, so its absence is something to fix here rather than at the first
// install attempt.
const allOk = computed(() => nodeOk.value && pnpmOk.value)
const belowRecommended = computed(() => store.runtime?.node_below_recommended ?? false)

// --- Adaptive install guidance ------------------------------------------------

/** Version floors and install commands as the backend computes them: the page
 *  then states the same numbers the resolver enforces, and offers the command
 *  the backend picked from the managers it actually detected. */
const requirements = computed(() => store.runtime?.requirements)

/** The node command to copy. The backend chose it from the detected managers —
 *  extending whichever the user already has — so the cascade lives there. */
const nodeCommand = computed(() => requirements.value?.node_install_command ?? '')

/** The pnpm command, likewise from the backend. Deliberately not version-pinned:
 *  the launcher accepts any pnpm at or above the floor, so pinning would go
 *  stale. */
const pnpmCommand = computed(() => requirements.value?.pnpm_install_command ?? '')

const copiedKey = ref('')

/** Copies via the shared helper (which reports the real outcome), and tracks
 *  which button to flip to「已复制」. */
async function copyCommand(text: string, key: string) {
  if (!(await copy(text))) return
  copiedKey.value = key
  window.setTimeout(() => {
    if (copiedKey.value === key) copiedKey.value = ''
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

/**
 * A toolchain installed in a terminal becomes visible as soon as the user
 * comes back to the window, so the page does not make them hunt for a button.
 * The store's refresh debounces nothing, so a focus storm costs one probe.
 */
async function onFocus() {
  if (checking.value) return
  await store.checkRuntime()
}

onMounted(() => {
  window.addEventListener('focus', onFocus)
})
onUnmounted(() => {
  window.removeEventListener('focus', onFocus)
})
</script>

<template>
  <div class="setup-page">
    <div class="dl-card setup-card">
      <div class="setup-icon">🛠️</div>
      <h2>{{ t('setup.title') }}</h2>
      <p class="setup-desc">{{ t('setup.desc') }}</p>

      <!-- Node status -->
      <div class="tool-row">
        <span class="tool-name">Node.js</span>
        <span class="tool-value">
          <a-tag v-if="nodeOk" color="green">
            {{ t('setup.installed', { v: node?.version ?? '' }) }}
          </a-tag>
          <a-tag v-else color="red">{{ t('setup.missing') }}</a-tag>
          <a-tag v-if="nodeOk && belowRecommended" color="orange">
            {{ t('setup.nodeBelowRecommended', { v: requirements?.recommended_node_major }) }}
          </a-tag>
        </span>
      </div>

      <!-- pnpm status -->
      <div class="tool-row">
        <span class="tool-name">pnpm</span>
        <span class="tool-value">
          <a-tag v-if="pnpmOk" color="green">
            {{ t('setup.installed', { v: pnpm?.version ?? '' }) }}
          </a-tag>
          <a-tag v-else color="red">{{ t('setup.missing') }}</a-tag>
        </span>
      </div>

      <!-- Where each tool resolves to: the launcher drives these exact paths. -->
      <div v-if="nodeOk || pnpmOk" class="resolved-paths">
        <p v-if="nodeOk && node?.path" class="resolved-path">
          <span class="resolved-label">node</span>
          <code>{{ node.path }}</code>
        </p>
        <p v-if="pnpmOk && pnpm?.path" class="resolved-path">
          <span class="resolved-label">pnpm</span>
          <code>{{ pnpm.path }}</code>
        </p>
      </div>

      <!-- Guidance: one block per missing tool, so a machine that lacks both is
           told both things at once. They used to be mutually exclusive (the
           pnpm block required nodeOk), which left a fresh machine with a Node
           command only — the user would install Node, assume they were done,
           and hit the missing-pnpm error when installing a version. -->
      <div v-if="!nodeOk" class="guide-block">
        <h4>{{ t('setup.installNode') }}</h4>
        <p class="guide-desc">{{ t('setup.nodeCommandDesc') }}</p>
        <div class="cmd-row">
          <pre class="cmd-text">{{ nodeCommand }}</pre>
          <button class="mac-secondary-btn" @click="copyCommand(nodeCommand, 'node')">
            {{ copiedKey === 'node' ? t('common.copied') : t('common.copy') }}
          </button>
        </div>
      </div>

      <div v-if="!pnpmOk" class="guide-block">
        <h4>{{ t('setup.installPnpm') }}</h4>
        <p class="guide-desc">{{ t('setup.pnpmCommandDesc', { major: requirements?.min_pnpm_major }) }}</p>
        <div class="cmd-row">
          <pre class="cmd-text">{{ pnpmCommand }}</pre>
          <button class="mac-secondary-btn" @click="copyCommand(pnpmCommand, 'pnpm')">
            {{ copiedKey === 'pnpm' ? t('common.copied') : t('common.copy') }}
          </button>
        </div>
      </div>

      <!-- Shown once for however many blocks are above, so the "come back and
           it re-checks itself" instruction is not repeated per tool. -->
      <p v-if="!allOk" class="guide-hint shared-hint">{{ t('setup.afterInstallHint') }}</p>

      <div v-if="allOk" class="guide-block ready-block">
        <a-result status="success" :title="t('setup.allReady')" />
        <a-button type="primary" @click="router.push({ name: 'home' })">
          {{ t('setup.enterApp') }}
        </a-button>
      </div>

      <div class="setup-actions">
        <a-button :loading="checking" type="outline" @click="recheck">
          {{ t('setup.recheck') }}
        </a-button>
      </div>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.setup-page {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

.setup-card {
  max-width: 560px;
  width: 100%;
  padding: 40px 48px;
}

.setup-icon {
  font-size: 44px;
  text-align: center;
}

h2 {
  margin: 12px 0 4px;
  text-align: center;
}

.setup-desc {
  color: var(--color-text-3);
  margin-bottom: 24px;
  text-align: center;
}

.tool-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 0;
  border-bottom: 1px dashed var(--color-border-2);

  .tool-name {
    font-weight: 600;
  }

  .tool-value {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
}

.resolved-paths {
  margin-top: 10px;
  text-align: left;
}

.resolved-path {
  display: flex;
  gap: 8px;
  align-items: baseline;
  margin: 4px 0;
  font-size: 12px;
  color: var(--color-text-3);

  .resolved-label {
    flex: 0 0 34px;
    font-weight: 600;
  }

  code {
    word-break: break-all;
    user-select: text;
  }
}

.guide-block {
  text-align: left;
  margin-top: 24px;
  padding: 16px;
  background: var(--color-fill-1);
  border-radius: 8px;

  h4 {
    margin: 0 0 8px;
  }
}

.ready-block {
  text-align: center;
}

.guide-desc {
  margin: 0 0 12px;
  color: var(--color-text-2);
}

.guide-hint {
  margin: 12px 0 0;
  font-size: 12px;
  color: var(--color-text-3);
}

/* The shared "it re-checks itself" line sits outside the guide blocks, so it
   needs its own inset to line up with their text. */
.shared-hint {
  margin: 10px 0 0;
  padding: 0 4px;
}

.cmd-row {
  display: flex;
  gap: 8px;
  align-items: flex-start;
}

.cmd-text {
  flex: 1;
  margin: 0;
  padding: 10px 14px;
  background: #1d2129;
  color: #a9b7c6;
  border-radius: 6px;
  font-family: Consolas, 'Courier New', monospace;
  font-size: 12px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
  user-select: text;
}

.setup-actions {
  margin-top: 24px;
  text-align: center;
}
</style>

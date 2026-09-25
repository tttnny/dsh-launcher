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
      <div class="setup-icon">
        <svg viewBox="0 0 48 48" width="42" height="42" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M30 6a9 9 0 0 0-8.5 12.1L8 31.6V40h8.4l13.5-13.5A9 9 0 1 0 30 6z" />
          <circle cx="33" cy="15" r="2.4" />
        </svg>
      </div>
      <h2>{{ t('setup.title') }}</h2>
      <p class="setup-desc">{{ t('setup.desc') }}</p>

      <!-- Toolchain status: inset group, one row per tool. -->
      <div class="apple-inset-group setup-tools">
        <div class="apple-inset-row">
          <div class="row-info">
            <span class="row-title">Node.js</span>
            <span v-if="nodeOk && node?.path" class="row-desc tnum">{{ node.path }}</span>
          </div>
          <span :class="['setup-tag', nodeOk ? 'ok' : 'missing']">
            {{ nodeOk ? t('setup.installed', { v: node?.version ?? '' }) : t('setup.missing') }}
          </span>
        </div>
        <div class="apple-inset-row">
          <div class="row-info">
            <span class="row-title">pnpm</span>
            <span v-if="pnpmOk && pnpm?.path" class="row-desc tnum">{{ pnpm.path }}</span>
          </div>
          <span :class="['setup-tag', pnpmOk ? 'ok' : 'missing']">
            {{ pnpmOk ? t('setup.installed', { v: pnpm?.version ?? '' }) : t('setup.missing') }}
          </span>
        </div>
      </div>

      <p v-if="nodeOk && belowRecommended" class="setup-note warn">
        {{ t('setup.nodeBelowRecommended', { v: requirements?.recommended_node_major }) }}
      </p>

      <!-- Guidance: one block per missing tool, so a machine that lacks both is
           told both things at once. -->
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

      <p v-if="!allOk" class="guide-hint shared-hint">{{ t('setup.afterInstallHint') }}</p>

      <div v-if="allOk" class="ready-block">
        <span class="ready-check">
          <svg viewBox="0 0 24 24" width="30" height="30" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="20 6 9 17 4 12" />
          </svg>
        </span>
        <p class="ready-title">{{ t('setup.allReady') }}</p>
        <button class="mac-primary-btn" @click="router.push({ name: 'home' })">
          {{ t('setup.enterApp') }}
        </button>
      </div>

      <div class="setup-actions">
        <button class="mac-secondary-btn" :disabled="checking" @click="recheck">
          {{ t('setup.recheck') }}
        </button>
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
  padding: 36px 40px;
}

.setup-icon {
  display: flex;
  justify-content: center;
  color: rgb(var(--primary-6));
  opacity: 0.9;
}

h2 {
  margin: 12px 0 4px;
  text-align: center;
}

.setup-desc {
  color: var(--color-text-3);
  margin: 0 0 22px;
  text-align: center;
  font-size: 13px;
  line-height: 1.6;
}

// Toolchain status rows (reuses the shared inset-group look).
.setup-tools {
  text-align: left;

  .row-desc {
    word-break: break-all;
    font-size: 11.5px;
  }
}

.setup-tag {
  flex-shrink: 0;
  padding: 2px 9px;
  font-size: 12px;
  font-weight: 600;
  border-radius: 6px;

  &.ok {
    background: rgb(var(--green-6) / 14%);
    color: rgb(var(--green-6));
  }

  &.missing {
    background: rgb(var(--red-6) / 14%);
    color: rgb(var(--red-6));
  }
}

.setup-note {
  margin: 10px 0 0;
  font-size: 12px;
  text-align: left;

  &.warn {
    color: rgb(var(--orange-6));
  }
}

.guide-block {
  text-align: left;
  margin-top: 18px;
  padding: 14px 16px;
  background: var(--apple-group-bg);
  border: 1px solid var(--apple-card-border);
  border-radius: 10px;

  h4 {
    margin: 0 0 6px;
    font-size: 13.5px;
  }
}

.guide-desc {
  margin: 0 0 10px;
  font-size: 12.5px;
  color: var(--color-text-3);
  line-height: 1.55;
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
  min-width: 0;
  margin: 0;
  padding: 10px 14px;
  background: #1d2129;
  color: #a9b7c6;
  border-radius: 8px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 12px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
  user-select: text;
}

// Ready state: a centered success mark instead of Arco's result block.
.ready-block {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  margin-top: 20px;
  padding: 18px;
  background: rgb(var(--green-6) / 8%);
  border: 1px solid rgb(var(--green-6) / 20%);
  border-radius: 10px;

  .ready-check {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    background: rgb(var(--green-6) / 15%);
    color: rgb(var(--green-6));
  }

  .ready-title {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text-1);
  }
}

.setup-actions {
  margin-top: 18px;
  text-align: center;
}
</style>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import { PLUGIN_ORIGIN_QUERY } from '@/composables/usePluginLink'
import type { BundleItem, ChangeOutcome, PluginView } from '@/api/types'
import DlIcon from '@/components/DlIcon.vue'
import BundleRow from '@/components/BundleRow.vue'
import DlEmpty from '@/components/DlEmpty.vue'

// The plugin manager is the DSH instance's own service, reached over
// `/api/pluginManager/*` — the launcher never writes a profile file
// (ADR-0001). What it manages is the **Bundle（补丁层）**: a whole patch layer,
// mounted or unmounted as a unit, which is what a profile selects and what
// carries a version, an availability and a removal rule. The composition rows
// inside a layer are DSH's own detail — the page states how many a layer
// contributes, and does not hand out a switch per row.
//
// Every write goes through the instance, and reports the truth it returned:
// applied / restart-required / overridden / failed / cancelled, or a refusal
// carrying the instance's own localized reason.

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useLauncherStore()

const homeId = computed(() =>
  typeof route.query.homeId === 'string' ? route.query.homeId : '',
)
const profile = computed(() =>
  typeof route.query.profile === 'string' ? route.query.profile : '',
)
const instanceId = computed(() =>
  typeof route.query.instanceId === 'string' ? route.query.instanceId : '',
)
const home = computed(() => store.homeById(homeId.value))
const instance = computed(() =>
  instanceId.value ? store.instanceById(instanceId.value) : undefined,
)

/**
 * A read-only page whose instance is known has exactly one way out — start it,
 * restart it (when it is up but we hold no token), or ask that machine once more
 * (when the launcher cannot reach the instance at all) — so the banner offers
 * that button instead of only naming the problem.
 *
 * 不可达 is not 已停止: a remote instance the launcher cannot reach must never be
 * handed a 启动 button, because pressing it is how a second DSH ends up running
 * on a server that was running one all along (ADR-0007). The retry asks the host
 * again, and it is the same action the instance rows offer for that state.
 */
const instanceState = computed(() =>
  instanceId.value ? store.statusOf(instanceId.value).state : 'stopped',
)
const unreachable = computed(() => instanceState.value === 'unreachable')
const canFix = computed(
  () =>
    !!instance.value && !!profile.value && instanceState.value !== 'starting',
)
const fixLabel = computed(() => {
  if (unreachable.value) return t('home.retryCheck')
  return instanceState.value === 'running'
    ? t('plugins.instanceRestart')
    : t('plugins.instanceStart')
})

const fixAction = useAction(async () => {
  const id = instanceId.value
  if (!id || !profile.value) return undefined
  if (unreachable.value) {
    await api.refreshInstanceStatus(id)
  } else if (instanceState.value === 'running') {
    await api.restartInstance(id)
  } else {
    await api.startInstance(id, profile.value)
  }
  return true
})

async function fixInstance() {
  const ok = await fixAction.run()
  if (ok !== true) return
  // The page flips to managed when the instance answers; give it a moment
  // before asking again so the click feels like it did something.
  await new Promise((r) => setTimeout(r, 1200))
  await load()
}

/**
 * "返回实例" goes back to the instance the user left, not to a fixed page: the
 * board's row (expanded, scrolled to) when the page was opened from that row,
 * or the instance's settings page when it was opened from there.
 */
function goBack() {
  if (instanceId.value) {
    if (
      route.query[PLUGIN_ORIGIN_QUERY] === 'instance-detail' &&
      instance.value
    ) {
      void router.push({
        name: 'instance-detail',
        params: { id: instanceId.value },
      })
      return
    }
    // The board focuses the row the user came from: same instance, same place.
    void router.push({ name: 'instances', query: { focus: instanceId.value } })
    return
  }
  void router.push({
    name: 'library',
    query: { tab: 'profiles', homeId: homeId.value },
  })
}

// --- The view snapshot --------------------------------------------------------

const view = ref<PluginView | null>(null)
const loading = ref(false)

const bundles = computed<BundleItem[]>(() => view.value?.bundles ?? [])
const mode = computed(() => view.value?.mode ?? 'not-running')
const managed = computed(() => mode.value === 'managed')
const reason = computed(() => view.value?.reason ?? null)
/** Nothing could be asked: not the instance, and not the files either. */
const unreadable = computed(() => mode.value === 'unreachable')

/**
 * The instance the page is working on. It is the one that answered, not the one
 * in the URL: the two differ whenever the row's instance is no longer running
 * that profile and another instance of the same HOME took over.
 */
const scopeInstanceId = computed(
  () => view.value?.instanceId ?? instanceId.value,
)
const scopeInstance = computed(() =>
  scopeInstanceId.value ? store.instanceById(scopeInstanceId.value) : undefined,
)

async function load() {
  view.value = null
  if (!homeId.value || !profile.value) return
  loading.value = true
  try {
    // The instance decides which process answers when two of them run this
    // HOME + profile at once, so the page hands over the one it was opened from.
    view.value = await api.listPlugins(
      homeId.value,
      profile.value,
      instanceId.value || undefined,
    )
  } catch (e) {
    Message.error(String(e))
  } finally {
    loading.value = false
  }
}

onMounted(() => void load())
watch([homeId, profile], () => void load())

// --- Write outcomes -----------------------------------------------------------

type Tone = 'success' | 'warning' | 'info'

function applicationOutcome(application: string): {
  tone: Tone
  label: string
} {
  switch (application) {
    case 'applied':
      return { tone: 'success', label: t('plugins.outcomeApplied') }
    case 'restart-required':
      return { tone: 'success', label: t('plugins.outcomeRestartRequired') }
    case 'overridden':
      return { tone: 'warning', label: t('plugins.outcomeOverridden') }
    case 'failed':
      return { tone: 'warning', label: t('plugins.outcomeFailed') }
    case 'cancelled':
      return { tone: 'info', label: t('plugins.outcomeCancelled') }
    default:
      return {
        tone: 'warning',
        label: t('plugins.outcomeUnknown', { application }),
      }
  }
}

/**
 * The one renderer for a write outcome, shared by the bundle toggle and the
 * bundle removal, so the refusal branch and the five `application` branches
 * cannot drift between them.
 *
 * A refusal (`changed:false`) carries the **localized** reason in `diagnostic`;
 * `errorCode` is the raw native code and is never shown.
 */
function renderOutcome(
  o: ChangeOutcome,
  success?: { applied: string; restartRequired: string },
): { tone: Tone; message: string } {
  if (!o.changed) {
    const why = o.diagnostic?.trim()
    return {
      tone: 'warning',
      message: why
        ? t('plugins.outcomeRefused', { reason: why })
        : t('plugins.outcomeNoReason'),
    }
  }
  const { tone, label } = applicationOutcome(o.application)
  if (o.application === 'applied' && success)
    return { tone, message: success.applied }
  if (o.application === 'restart-required' && success) {
    return { tone, message: success.restartRequired }
  }
  if (o.application === 'failed') {
    const detail = o.diagnostic?.trim()
    if (detail) return { tone, message: `${label}：${detail}` }
  }
  return { tone, message: label }
}

function showOutcome({ tone, message }: { tone: Tone; message: string }) {
  if (tone === 'success') Message.success(message)
  else if (tone === 'info') Message.info(message)
  else Message.warning(message)
}

/**
 * A layer write the instance answered with `restart-required` is on disk but
 * not in the process: the page keeps saying so, with the one button that
 * applies it, instead of leaving the user to remember a toast.
 */
const restartNeeded = ref(false)

function reportOutcome(
  outcome: ChangeOutcome,
  success?: { applied: string; restartRequired: string },
) {
  if (outcome.changed && outcome.application === 'restart-required')
    restartNeeded.value = true
  showOutcome(renderOutcome(outcome, success))
}

// Re-read when an instance of this HOME starts or stops: the page switches
// between managed and read-only without an app restart.
const statusTick = computed(() => {
  if (!homeId.value) return ''
  return store.instances
    .filter((i) => i.home_id === homeId.value)
    .map((i) => {
      const st = store.statusOf(i.id)
      return `${i.id}:${st.state}:${st.profile ?? ''}`
    })
    .join('|')
})
watch(statusTick, (now, before) => {
  if (now !== before) void load()
})

/**
 * Any change to the process this page is talking to — it went down, or came
 * back with a new URL — means it is no longer the process that answered
 * `restart-required`, so the notice has done its job.
 */
const liveTick = computed(() => {
  if (!instanceId.value) return ''
  const st = store.statusOf(instanceId.value)
  return `${st.state}:${st.url ?? ''}`
})
watch(liveTick, (now, before) => {
  if (now !== before && before) restartNeeded.value = false
})

// --- Bundles ------------------------------------------------------------------

/**
 * The page is read top-down by what a layer *is to you*, and the split is the
 * one property that does not change while you are looking at it — whether you
 * could take the layer off at all:
 *
 *   * 你添加的 — removable: the layers you added, always in view;
 *   * 随 DSH 发布 — not removable: they come with the DSH install, mostly the
 *     core and its official optional layers, so they are folded away.
 *
 * Mounted or not is *state*, not a section: the switch says it, and the dot
 * says it without reading. That is deliberate — a fold keyed on `enabled`
 * would move a row out from under the pointer that just toggled it.
 */
const ownLayers = computed(() => bundles.value.filter((b) => b.removable))
const shippedLayers = computed(() => bundles.value.filter((b) => !b.removable))
const mountedCount = computed(
  () => bundles.value.filter((b) => b.enabled).length,
)

const groups = computed(() =>
  (
    [
      {
        key: 'mine',
        tone: 'own' as const,
        label: t('plugins.bundlesMineGroup'),
        items: ownLayers.value,
      },
      {
        key: 'shipped',
        tone: 'shipped' as const,
        label: t('plugins.bundlesShippedGroup'),
        items: shippedLayers.value,
      },
    ] as const
  ).filter((g) => g.items.length > 0 || g.key === 'mine'),
)

// Only the layers you added are open: they are the ones you came in to edit.
const expandedGroups = ref<Record<string, boolean>>({
  mine: true,
  shipped: false,
})
function toggleGroup(key: string) {
  expandedGroups.value = {
    ...expandedGroups.value,
    [key]: !expandedGroups.value[key],
  }
}

const bundlePending = ref<Record<string, boolean>>({})
/** Any write in flight: every control on the page locks (see `BundleRow.locked`). */
const locked = computed(() => Object.keys(bundlePending.value).length > 0)

function bundleName(b: BundleItem): string {
  return b.title?.trim() || b.name
}

async function toggleBundle(b: BundleItem, enabled: boolean) {
  if (!homeId.value || !profile.value) return
  bundlePending.value = { ...bundlePending.value, [b.name]: true }
  try {
    const outcome = await api.setBundleEnabled({
      homeId: homeId.value,
      profile: profile.value,
      instanceId: instanceId.value || undefined,
      name: b.name,
      enabled,
    })
    reportOutcome(outcome, {
      applied: t('plugins.bundleApplied', { name: bundleName(b) }),
      restartRequired: t('plugins.bundleAppliedRestart', {
        name: bundleName(b),
      }),
    })
    await load()
  } catch (e) {
    Message.error(String(e))
    await load()
  } finally {
    const next = { ...bundlePending.value }
    delete next[b.name]
    bundlePending.value = next
  }
}

function canRemove(b: BundleItem): boolean {
  return managed.value && b.removable && !bundlePending.value[b.name]
}

async function removeBundle(b: BundleItem) {
  if (!homeId.value || !profile.value) return
  bundlePending.value = { ...bundlePending.value, [b.name]: true }
  try {
    const outcome = await api.removeBundle({
      homeId: homeId.value,
      profile: profile.value,
      instanceId: instanceId.value || undefined,
      name: b.name,
    })
    reportOutcome(outcome, {
      applied: t('plugins.bundleRemoved', { name: bundleName(b) }),
      restartRequired: t('plugins.bundleRemovedRestart', {
        name: bundleName(b),
      }),
    })
    await load()
  } catch (e) {
    Message.error(String(e))
    await load()
  } finally {
    const next = { ...bundlePending.value }
    delete next[b.name]
    bundlePending.value = next
  }
}
</script>

<template>
  <div class="dl-page">
    <header class="dl-head">
      <div class="dl-head-text">
        <h1>{{ t('plugins.pageTitle') }}</h1>
        <p class="dl-head-sub plugin-scope">
          <span
            v-if="scopeInstance"
            class="dl-chip"
            :title="scopeInstance.name"
          >
            {{ scopeInstance.name }}
          </span>
          <span v-if="scopeInstance" class="plugin-scope-sep">/</span>
          <span class="dl-chip">{{ home?.name ?? '—' }}</span>
          <span class="plugin-scope-sep">/</span>
          <span class="dl-chip dl-mono">{{ profile || '—' }}</span>
        </p>
      </div>
      <div class="dl-head-actions">
        <button class="dl-btn dl-btn--quiet" @click="goBack">
          <DlIcon name="back" :size="13" />
          {{
            instance ? t('plugins.backToInstance') : t('plugins.backToProfiles')
          }}
        </button>
      </div>
    </header>

    <div v-if="!homeId || !profile" class="dl-surface">
      <DlEmpty icon="plugin" :description="t('plugins.pluginsPickProfile')" />
    </div>

    <template v-else>
      <!-- Read-only: no instance is running this profile, the one that is
           cannot be authenticated, or it is unreachable. The reason is
           backend-localized, and the button is the way out of the state. -->
      <div v-if="!managed && reason" class="readonly-banner">
        <DlIcon name="alert" :size="14" />
        <span>{{ reason }}</span>
        <span class="readonly-spacer" />
        <button
          v-if="canFix"
          class="dl-btn dl-btn--quiet readonly-fix"
          :disabled="fixAction.busy['*']"
          @click="fixInstance"
        >
          <DlIcon name="restart" :size="13" />
          {{ fixLabel }}
        </button>
      </div>

      <!-- Written, but not yet true: the layers the instance is running are the
           old ones until it comes back. -->
      <div v-else-if="restartNeeded" class="restart-banner">
        <DlIcon name="restart" :size="14" />
        <span>{{ t('plugins.restartNeeded') }}</span>
        <span class="readonly-spacer" />
        <button
          v-if="instanceId"
          class="dl-btn dl-btn--quiet"
          :disabled="fixAction.busy['*']"
          @click="fixInstance"
        >
          <DlIcon name="restart" :size="13" />
          {{ t('plugins.instanceRestart') }}
        </button>
        <button
          class="dl-icon-btn"
          :title="t('common.close')"
          @click="restartNeeded = false"
        >
          <DlIcon name="close" :size="12" />
        </button>
      </div>

      <!-- 补丁层: the whole page, and only while the instance is answering. A
           layer is mounted or unmounted as a unit; the rows inside it are
           reported as a count, not managed one by one. -->
      <section v-if="managed" class="dl-section">
        <div class="dl-section-label">
          {{ t('plugins.bundlesTitle') }}
          <span class="dl-section-count">
            {{
              t('plugins.bundlesMountedOf', {
                mounted: mountedCount,
                total: bundles.length,
              })
            }}
          </span>
        </div>

        <div class="dl-surface bundle-list">
          <div v-if="bundles.length === 0" class="dl-row-none">
            {{ loading ? t('common.loading') : t('plugins.bundlesEmpty') }}
          </div>

          <template v-else>
            <section v-for="g in groups" :key="g.key" class="bundle-group">
              <button
                class="group-head"
                type="button"
                @click="toggleGroup(g.key)"
              >
                <DlIcon
                  name="chevron-down"
                  :size="13"
                  class="group-chev"
                  :class="{ 'is-open': expandedGroups[g.key] }"
                />
                {{ g.label }}
                <span class="group-count dl-tnum">{{ g.items.length }}</span>
              </button>
              <template v-if="expandedGroups[g.key]">
                <div v-if="g.items.length === 0" class="dl-row-none">
                  {{ t('plugins.bundlesNoneMine') }}
                </div>
                <BundleRow
                  v-for="b in g.items"
                  :key="b.name"
                  :tone="g.tone"
                  :bundle="b"
                  :pending="!!bundlePending[b.name]"
                  :managed="managed"
                  :locked="locked"
                  @toggle="(on) => toggleBundle(b, on)"
                  @remove="removeBundle(b)"
                />
              </template>
            </section>
          </template>
        </div>

        <p class="bundle-hint">{{ t('plugins.bundleOrderHint') }}</p>
      </section>

      <!-- Read-only: the layers the profile's own files declare. No folds and
           no grouping — grouping is `removable`, and a file cannot prove that,
           so this is a flat list of facts with the switches locked. -->
      <section v-else class="dl-section">
        <div class="dl-section-label">
          {{ t('plugins.offlineTitle') }}
          <span v-if="bundles.length" class="dl-section-count">
            {{
              t('plugins.bundlesMountedOf', {
                mounted: mountedCount,
                total: bundles.length,
              })
            }}
          </span>
        </div>

        <div class="dl-surface bundle-list">
          <div v-if="bundles.length === 0" class="dl-row-none">
            {{
              loading
                ? t('common.loading')
                : unreadable
                  ? t('plugins.offlineUnreadable')
                  : t('plugins.offlineEmpty')
            }}
          </div>
          <template v-else>
            <BundleRow
              v-for="b in bundles"
              :key="b.name"
              tone="own"
              :bundle="b"
              :pending="false"
              :managed="false"
              :locked="false"
            />
          </template>
        </div>

        <!-- The projection's own note belongs on a page that read the profile's
             files. In the unreachable case nothing may have been read at all,
             and the instance is not 「没在跑」 — so the note's 「启动实例后」 is the
             one action this state must not offer. The banner above says what
             happened and how to leave it. -->
        <p v-if="!unreadable" class="bundle-hint">
          {{ t('plugins.offlineNote') }}
        </p>
      </section>
    </template>
  </div>
</template>

<style lang="scss" scoped>
.plugin-scope {
  display: flex;
  align-items: center;
  gap: 6px;
}

.plugin-scope-sep {
  color: var(--dl-ink-4);
}

// Both banners say "the list below is not the whole truth", and both hand over
// the one action that fixes it: one is blocked (warn), one is pending (signal).
.readonly-banner,
.restart-banner {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 16px;
  padding: 10px 14px;
  border: 1px solid var(--dl-line);
  border-radius: var(--dl-r-ctl);
  color: var(--dl-ink-2);
  font-size: var(--dl-text-sm);
  line-height: 1.5;

  svg {
    flex-shrink: 0;
  }
}

.readonly-banner {
  border-left: 3px solid var(--dl-warn);
  background: var(--dl-warn-soft);

  svg {
    color: var(--dl-warn);
  }
}

.restart-banner {
  border-left: 3px solid var(--dl-signal);
  background: var(--dl-signal-soft);

  svg {
    color: var(--dl-signal);
  }
}

.readonly-spacer {
  flex: 1;
}

// The banner is a dead end without this: it names the state and hands over the
// one action that leaves it.
.readonly-fix {
  flex-shrink: 0;
  background: var(--dl-surface);
}

.bundle-list {
  overflow: hidden;
}

// A folded group's header: the count is the whole point, so it is always
// visible — the layers behind it are what folds.
.group-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 10px 16px;
  border: none;
  border-top: 1px solid var(--dl-line);
  background: var(--dl-surface-2);
  color: var(--dl-ink-3);
  font-family: inherit;
  font-size: var(--dl-text-sm);
  font-weight: 550;
  text-align: left;
  cursor: pointer;

  &:first-child {
    border-top: none;
  }

  &:hover {
    color: var(--dl-ink);
  }

  .group-count {
    color: var(--dl-ink-4);
    font-weight: 400;
  }
}

.group-chev {
  flex-shrink: 0;
  transition: transform var(--dl-slow) var(--dl-ease);

  &.is-open {
    transform: rotate(180deg);
  }
}

.bundle-hint {
  margin: 8px 2px 0;
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
}
</style>

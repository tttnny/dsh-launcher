<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type {
  BundleItem,
  ChangeOutcome,
  DshInstance,
  PluginView,
} from '@/api/types'
import { useLauncherStore } from '@/stores/launcher'
import { useProfiles } from '@/composables/useProfiles'
import { useAction } from '@/composables/useAction'
import DlIcon from '@/components/DlIcon.vue'
import BundleRow from '@/components/BundleRow.vue'

// 插件管理是这个实例自己的服务（/api/pluginManager/*），启动器从不改写 Profile
// 文件。它管理的单位是「补丁层」：整体挂载或卸载，带版本、可用性与移除规则；
// 层内部的条目是 DSH 自己的组合细节，只报数量，不逐个给开关。
//
// 每一次改动都经过实例，并如实呈现它返回的结果：已生效 / 需重启 / 被覆盖 /
// 失败 / 已取消，或者一个带本地化原因的拒绝。

const props = defineProps<{ instance: DshInstance }>()

const store = useLauncherStore()
const { loadForHome } = useProfiles()

const view = ref<PluginView | null>(null)
const loading = ref(false)
const loadError = ref('')
const profileList = ref<string[]>([])
const profilesLoaded = ref(false)
const profileError = ref('')

const status = computed(() => store.statusOf(props.instance.id))
const mode = computed(() => view.value?.mode ?? '')
const managed = computed(() => mode.value === 'managed')
const bundles = computed<BundleItem[]>(() => view.value?.bundles ?? [])
const reason = computed(() => view.value?.reason ?? '')

/**
 * 插件列表要按一个 Profile 去问。实例正在运行时以它实际的配置为准；没有在运行时
 * 落回默认配置、上次使用的配置，再落回数据目录里的第一个。
 */
const currentProfile = computed(() => {
  const st = status.value
  if (st.profile) return st.profile
  const inst = props.instance
  const list = profileList.value
  if (profilesLoaded.value) {
    if (inst.last_profile && list.includes(inst.last_profile))
      return inst.last_profile
    if (inst.default_profile && list.includes(inst.default_profile))
      return inst.default_profile
    return list[0] ?? ''
  }
  // 配置列表读不到时，仍拿记录里已知的名字去问，好让主机不可达这类状态能显示出来。
  return inst.default_profile ?? inst.last_profile ?? ''
})

async function load() {
  loadError.value = ''
  profileError.value = ''
  view.value = null
  loading.value = true
  try {
    const list = await loadForHome(props.instance.home_id)
    if (list) {
      profileList.value = list
      profilesLoaded.value = true
    } else {
      profileError.value = '读不到这个数据目录的配置'
    }
    if (!currentProfile.value) return
    view.value = await api.listPlugins(
      props.instance.home_id,
      currentProfile.value,
      props.instance.id,
    )
  } catch (e) {
    loadError.value = String(e)
  } finally {
    loading.value = false
  }
}

onMounted(() => void load())

// 实例的状态或它运行的配置一变，这个面板就要重新问一次：它决定的是 managed
// 还是只读。
const statusKey = computed(
  () => `${status.value.state}:${status.value.profile ?? ''}`,
)
watch(statusKey, () => void load())
watch(
  () => props.instance.id,
  () => {
    profilesLoaded.value = false
    profileList.value = []
    void load()
  },
)

// --- 从只读状态里出来 --------------------------------------------------------

const fixAction = useAction(async () => {
  const st = status.value
  const id = props.instance.id
  if (st.state === 'unreachable') {
    await api.refreshInstanceStatus(id)
    return true
  }
  if (st.state === 'running' || st.state === 'starting') {
    await api.restartInstance(id)
    return true
  }
  const profile = currentProfile.value
  if (!profile)
    throw new Error('这个数据目录还没有 Profile，先在「数据」页创建配置')
  await api.startInstance(id, profile)
  return true
})

async function fix() {
  const ok = await fixAction.run()
  if (ok !== true) return
  await new Promise((r) => setTimeout(r, 1200))
  await load()
}

const fixLabel = computed(() => {
  const st = status.value
  if (mode.value === 'unreachable' || st.state === 'unreachable') return '重试'
  if (
    mode.value === 'adopted' ||
    st.state === 'running' ||
    st.state === 'starting'
  )
    return '重启实例'
  return '启动实例'
})

const bannerTitle = computed(() => {
  switch (mode.value) {
    case 'unreachable':
      return '主机不可达'
    case 'adopted':
      return '需要重启实例'
    default:
      return '实例没有在运行'
  }
})

const bannerText = computed(() => {
  if (reason.value) return reason.value
  switch (mode.value) {
    case 'unreachable':
      return '这台机器现在连不上，读不到这个实例的插件；恢复连接后点「重试」。'
    case 'adopted':
      return '这个实例的插件管理还没有交给启动器，重启一次即可接管。'
    default:
      return '启动这个实例后，才能在这里管理它的补丁层。'
  }
})

const showBanner = computed(
  () => !!view.value && !managed.value && !loading.value,
)

/** 数据目录里一个 Profile 都没有：没有可问的对象，启动也无从谈起。 */
const noProfile = computed(
  () =>
    !loading.value &&
    !view.value &&
    !loadError.value &&
    !profileError.value &&
    !currentProfile.value,
)

// --- 写入结果 ---------------------------------------------------------------

type Tone = 'success' | 'warning' | 'info'

function applicationOutcome(application: string): {
  tone: Tone
  label: string
} {
  switch (application) {
    case 'applied':
      return { tone: 'success', label: '已生效' }
    case 'restart-required':
      return { tone: 'success', label: '需重启该实例后生效' }
    case 'overridden':
      return {
        tone: 'warning',
        label: '已被更高优先级的层覆盖，本次改动不生效',
      }
    case 'failed':
      return { tone: 'warning', label: '改动已记录但未能生效' }
    case 'cancelled':
      return { tone: 'info', label: '已取消' }
    default:
      return { tone: 'warning', label: `实例返回了未知结果：${application}` }
  }
}

/**
 * 一个写入结果的唯一渲染处，启停与移除共用，拒绝分支和五种 application
 * 分支不会在两者之间走样。拒绝（changed:false）把本地化原因放在 diagnostic，
 * errorCode 只是原始代码，不展示。
 */
function renderOutcome(
  o: ChangeOutcome,
  success?: { applied: string; restartRequired: string },
): { tone: Tone; message: string } {
  if (!o.changed) {
    const why = o.diagnostic?.trim()
    return { tone: 'warning', message: why ? `未能改动：${why}` : '未能改动' }
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

/** 实例答了「需重启」：改动已落盘但进程里还没生效，页面一直说清这件事。 */
const restartNeeded = ref(false)

function reportOutcome(
  outcome: ChangeOutcome,
  success?: { applied: string; restartRequired: string },
) {
  if (outcome.changed && outcome.application === 'restart-required')
    restartNeeded.value = true
  showOutcome(renderOutcome(outcome, success))
}

// 进程一变（停了、或带着新地址回来了），它就不再是那个回答过「需重启」的进程。
const liveTick = computed(
  () => `${status.value.state}:${status.value.url ?? ''}`,
)
watch(liveTick, (now, before) => {
  if (now !== before && before) restartNeeded.value = false
})

// --- 补丁层 -----------------------------------------------------------------

const ownLayers = computed(() => bundles.value.filter((b) => b.removable))
const shippedLayers = computed(() => bundles.value.filter((b) => !b.removable))
const mountedCount = computed(
  () => bundles.value.filter((b) => b.enabled).length,
)
const shippedOpen = ref(false)

const bundlePending = ref<Record<string, boolean>>({})
const locked = computed(() => Object.keys(bundlePending.value).length > 0)

function bundleName(b: BundleItem): string {
  return b.title?.trim() || b.name
}

async function toggleBundle(b: BundleItem, enabled: boolean) {
  bundlePending.value = { ...bundlePending.value, [b.name]: true }
  try {
    const outcome = await api.setBundleEnabled({
      homeId: props.instance.home_id,
      profile: currentProfile.value,
      instanceId: props.instance.id,
      name: b.name,
      enabled,
    })
    reportOutcome(outcome, {
      applied: `已更新补丁层「${bundleName(b)}」`,
      restartRequired: `已更新补丁层「${bundleName(b)}」，需重启实例后生效`,
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

async function removeBundle(b: BundleItem) {
  bundlePending.value = { ...bundlePending.value, [b.name]: true }
  try {
    const outcome = await api.removeBundle({
      homeId: props.instance.home_id,
      profile: currentProfile.value,
      instanceId: props.instance.id,
      name: b.name,
    })
    reportOutcome(outcome, {
      applied: `已移除补丁层「${bundleName(b)}」`,
      restartRequired: `已移除补丁层「${bundleName(b)}」，需重启实例后生效`,
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
  <section class="section plugin-panel">
    <div v-if="loading && !view" class="surface surface--pad muted">
      正在读取插件…
    </div>

    <div v-else-if="profileError && !view" class="notice notice--danger">
      <DlIcon name="alert" :size="14" />
      <div class="notice-body">
        <div class="notice-title">读不到这个数据目录的配置</div>
        <div>{{ profileError }}</div>
      </div>
      <button class="btn btn--quiet btn--sm" @click="load">重试</button>
    </div>

    <div v-else-if="loadError" class="notice notice--danger">
      <DlIcon name="alert" :size="14" />
      <div class="notice-body">
        <div class="notice-title">读不到插件列表</div>
        <div>{{ loadError }}</div>
      </div>
      <button class="btn btn--quiet btn--sm" @click="load">重试</button>
    </div>

    <div v-else-if="noProfile" class="notice notice--info">
      <DlIcon name="profile" :size="14" />
      <div class="notice-body">
        <div class="notice-title">这个数据目录还没有 Profile</div>
      </div>
    </div>

    <template v-else>
      <!-- 只读：实例没在运行、没有把管理权交给启动器、或那台机器连不上。
           原因由后端本地化，按钮就是离开这个状态的那一步。 -->
      <div
        v-if="showBanner"
        class="notice"
        :class="mode === 'unreachable' ? 'notice--warn' : 'notice--info'"
      >
        <DlIcon name="alert" :size="14" />
        <div class="notice-body">
          <div class="notice-title">{{ bannerTitle }}</div>
        </div>
        <button
          class="btn btn--quiet btn--sm"
          :disabled="fixAction.busy['*']"
          @click="fix"
        >
          <DlIcon name="refresh" :size="13" />
          {{ fixLabel }}
        </button>
      </div>

      <!-- 已写盘但进程里还没生效：一直提示，直到实例重启。 -->
      <div v-if="restartNeeded && managed" class="notice notice--info">
        <DlIcon name="restart" :size="14" />
        <div class="notice-body">
          <div class="notice-title">补丁层的改动要重启实例后才生效</div>
        </div>
        <button
          class="btn btn--quiet btn--sm"
          :disabled="fixAction.busy['*']"
          @click="fix"
        >
          重启实例
        </button>
        <button class="icon-btn" title="关闭" @click="restartNeeded = false">
          <DlIcon name="close" :size="12" />
        </button>
      </div>

      <!-- 可管理：完整的分组列表与开关。 -->
      <template v-if="managed">
        <div class="section-head">
          <span class="section-title">补丁层</span>
          <span class="section-hint tnum">
            已启用 {{ mountedCount }} / {{ bundles.length }}
          </span>
        </div>

        <div class="surface">
          <div class="group-head">
            你添加的
            <span class="group-count tnum">{{ ownLayers.length }}</span>
          </div>
          <div v-if="ownLayers.length === 0" class="group-empty">
            没有你自己添加的补丁层
          </div>
          <BundleRow
            v-for="b in ownLayers"
            :key="b.name"
            tone="own"
            :bundle="b"
            :pending="!!bundlePending[b.name]"
            :managed="managed"
            :locked="locked"
            @toggle="(on) => toggleBundle(b, on)"
            @remove="removeBundle(b)"
          />

          <template v-if="shippedLayers.length">
            <button
              class="group-head is-toggle"
              type="button"
              @click="shippedOpen = !shippedOpen"
            >
              <DlIcon
                name="chevron-down"
                :size="13"
                class="group-chev"
                :class="{ 'is-open': shippedOpen }"
              />
              随 DSH 发布
              <span class="group-count tnum">{{ shippedLayers.length }}</span>
            </button>
            <template v-if="shippedOpen">
              <BundleRow
                v-for="b in shippedLayers"
                :key="b.name"
                tone="shipped"
                :bundle="b"
                :pending="!!bundlePending[b.name]"
                :managed="managed"
                :locked="locked"
                @toggle="(on) => toggleBundle(b, on)"
                @remove="removeBundle(b)"
              />
            </template>
          </template>
        </div>
      </template>

      <!-- 只读但读到了 Profile 自己声明的层：平铺展示，开关锁住。 -->
      <template v-else-if="bundles.length">
        <div class="section-head">
          <span class="section-title">这个 Profile 装了什么</span>
          <span class="section-hint tnum">
            已启用 {{ mountedCount }} / {{ bundles.length }}
          </span>
        </div>
        <div class="surface">
          <BundleRow
            v-for="b in bundles"
            :key="b.name"
            tone="own"
            :bundle="b"
            :pending="false"
            :managed="false"
            :locked="false"
          />
        </div>
      </template>
    </template>
  </section>
</template>

<style lang="scss" scoped>
.plugin-panel .notice + .notice {
  margin-top: 12px;
}

.plugin-panel .notice + .section-head,
.plugin-panel .notice + .surface {
  margin-top: 16px;
}

.group-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 9px 16px;
  border: none;
  border-top: 1px solid var(--dl-line);
  background: var(--dl-surface-2);
  color: var(--dl-ink-3);
  font-family: inherit;
  font-size: var(--dl-text-sm);
  font-weight: 600;
  text-align: left;

  &:first-child {
    border-top: none;
  }
}

.group-head.is-toggle {
  cursor: pointer;

  &:hover {
    color: var(--dl-ink);
  }
}

.group-count {
  color: var(--dl-ink-4);
  font-weight: 400;
}

.group-chev {
  flex-shrink: 0;
  transition: transform var(--dl-slow) var(--dl-ease);

  &.is-open {
    transform: rotate(180deg);
  }
}

.group-empty {
  padding: 12px 16px;
  border-top: 1px solid var(--dl-line);
  color: var(--dl-ink-4);
  font-size: var(--dl-text-sm);
}

.plugin-hint {
  margin: 8px 2px 0;
  font-size: var(--dl-text-sm);
  line-height: 1.5;
  color: var(--dl-ink-3);
}
</style>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'

// Creating an instance is one decision with three parts: a name, an installed
// version, and the DSH_HOME that version reads. Installing a version already
// makes one instance, so this sheet is how a *second* instance of the same
// version — or the same version on another HOME — gets made.

const props = defineProps<{
  visible: boolean
  /**
   * Pre-fills the sheet from an existing instance ("duplicate"). The profile
   * is not part of it: a copy is a new binding, and the profile is picked at
   * launch time. `hostId` comes along because a copy binds on the machine the
   * original runs on — the sheet then offers that machine's versions and
   * DSH_HOMEs, not the current host's.
   */
  preset?: {
    name: string
    versionId: string
    homeId: string
    hostId?: string
  } | null
}>()
const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'created', id: string): void
}>()

const store = useLauncherStore()

const name = ref('')
const versionId = ref<string | undefined>(undefined)
const homeId = ref<string | undefined>(undefined)

/**
 * The machine this sheet binds on: the instance being copied, or 当前主机. An
 * instance is one binding on one host (ADR-0006 §1), so both selects list that
 * host's resources only — an impossible combination is not offered in the first
 * place rather than refused on submit.
 */
const hostId = computed(() => props.preset?.hostId ?? store.currentHostId)
const host = computed(() => store.hostById(hostId.value))
const versions = computed(() => store.versionsByHost[hostId.value] ?? [])
const homes = computed(() => store.homesByHost[hostId.value] ?? [])
/**
 * Why this machine's versions and DSH_HOMEs could not be read. The two selects
 * list **one machine's** resources, so that machine has to answer for them: a
 * failed read leaves the store with no list at all (`loadHostResources`), and
 * this sheet says what went wrong instead of offering yesterday's list as
 * today's — or, worse, 「还没有装任何版本」 about a machine that never answered.
 */
const hostError = computed(
  () =>
    store.versionErrors[hostId.value] || store.homeErrors[hostId.value] || '',
)

function retryHost() {
  void store.loadHostResources(hostId.value)
}

/** `base`, then `base 2`, `base 3`… — instance names are unique **on their own
 *  host** (ADR-0006 §1), so only this machine's names are in the way: `web` on
 *  another machine says nothing about `web` here. */
function uniqueName(base: string): string {
  const existing = new Set(
    store.instancesOfHost(hostId.value).map((i) => i.name),
  )
  if (!existing.has(base)) return base
  let n = 2
  while (existing.has(`${base} ${n}`)) n += 1
  return `${base} ${n}`
}

// Defaults follow the common case: a name that cannot collide, and the version
// the last install produced.
function reset() {
  const preset = props.preset
  if (preset) {
    name.value = uniqueName(`${preset.name} 副本`)
    versionId.value = preset.versionId
    homeId.value = preset.homeId
    return
  }
  const onThisHost = store.instancesOfHost(hostId.value)
  let n = onThisHost.length + 1
  const existing = new Set(onThisHost.map((i) => i.name))
  while (existing.has(`${`实例 `}${n}`)) n += 1
  name.value = `${`实例 `}${n}`
  versionId.value = versions.value[0]?.id
  homeId.value = homes.value[0]?.id
}

watch(
  () => props.visible,
  (v) => {
    if (v) reset()
  },
)

// A new instance is a fresh binding, so it can only use a version that is
// already installed — installing is the Versions tab's job, not this sheet's.
const noVersions = computed(() => versions.value.length === 0)
const noHomes = computed(() => homes.value.length === 0)

const createAction = useAction(
  // The instance is bound on the machine whose versions and DSH_HOMEs the two
  // selects are listing — the current host (ADR-0006 §3: a binding never spans
  // two machines).
  () =>
    api.createInstance(
      name.value.trim(),
      versionId.value!,
      homeId.value!,
      hostId.value,
    ),
  { success: () => `实例已创建` },
)
const saving = computed(() => createAction.busy['*'])

const valid = computed(
  () =>
    !!name.value.trim() &&
    !!versionId.value &&
    !!homeId.value &&
    !noVersions.value &&
    !noHomes.value,
)

async function onCreate() {
  if (!valid.value) return
  const inst = await createAction.run()
  if (!inst) return
  await store.refreshInstances()
  emit('created', inst.id)
  emit('update:visible', false)
}

function close() {
  emit('update:visible', false)
}
</script>

<template>
  <a-modal
    :visible="visible"
    :mask-closable="true"
    width="440px"
    modal-class="dl-sheet"
    @update:visible="(v) => emit('update:visible', v)"
  >
    <template #title>{{ preset ? `复制实例` : `新建实例` }}</template>

    <!-- The machine has to answer before the sheet can offer anything: a host
         whose read failed gets the reason and a retry, never a stale list. -->
    <div v-if="hostError" class="dl-surface dl-failure">
      <p class="dl-failure-title">{{ `读不到这台主机的资源` }}</p>
      <p class="dl-failure-text">{{ hostError }}</p>
      <p class="dl-failure-hint">
        {{ `上面是那台机器回的话。修好之后再试一次。` }}
      </p>
      <div class="sheet-footer" style="margin-top: 8px">
        <button class="dl-btn" @click="close">{{ `关闭` }}</button>
        <button class="dl-btn dl-btn--primary" @click="retryHost">
          {{ `刷新` }}
        </button>
      </div>
    </div>

    <!-- Nothing installed yet: the sheet cannot do its job, so it says which
         page can, instead of offering three empty selects. -->
    <div v-else-if="noVersions" class="dl-empty">
      <p class="dl-empty-title">{{ `还没有安装任何 DSH 版本` }}</p>
      <p class="dl-empty-text">
        {{ `先在资源库里安装一个版本，再回来创建实例。` }}
      </p>
      <button
        class="dl-btn dl-btn--primary"
        style="margin-top: 6px"
        @click="close"
      >
        {{ `关闭` }}
      </button>
    </div>

    <div v-else class="new-instance-body">
      <p v-if="host && !host.builtin" class="dl-field-hint">
        {{
          `这台机器的资源只属于它自己：下面只能选「${host.display_name}」上的 DSH 版本与 DSH_HOME。`
        }}
      </p>
      <div class="dl-field">
        <label class="dl-field-label">{{ `实例名称` }}</label>
        <input
          v-model="name"
          class="dl-input"
          :placeholder="`例如：主实例`"
          @keydown.enter="onCreate"
        />
      </div>

      <div class="dl-field">
        <label class="dl-field-label">{{ `DSH 版本` }}</label>
        <a-select v-model="versionId" style="width: 100%">
          <a-option v-for="v in versions" :key="v.id" :value="v.id">{{
            v.version
          }}</a-option>
        </a-select>
      </div>

      <div class="dl-field">
        <label class="dl-field-label">{{ `DSH_HOME` }}</label>
        <a-select v-model="homeId" style="width: 100%">
          <a-option v-for="h in homes" :key="h.id" :value="h.id">
            {{ h.name }} — {{ h.path }}
          </a-option>
        </a-select>
        <p class="dl-field-hint">
          {{ `同一版本可以绑定到不同的 DSH_HOME，或与其他实例共用一个。` }}
        </p>
      </div>
    </div>

    <template v-if="!noVersions" #footer>
      <div class="sheet-footer">
        <button class="dl-btn dl-btn--quiet" @click="close">
          {{ `取消` }}
        </button>
        <button
          class="dl-btn dl-btn--primary"
          :disabled="!valid || saving"
          @click="onCreate"
        >
          {{ `创建` }}
        </button>
      </div>
    </template>
  </a-modal>
</template>

<style lang="scss" scoped>
.new-instance-body {
  padding-top: 2px;
}

.sheet-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>

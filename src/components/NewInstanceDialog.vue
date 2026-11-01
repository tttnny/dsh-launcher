<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { api } from '@/api'
import { useLauncherStore } from '@/stores/launcher'
import { useAction } from '@/composables/useAction'
import DlIcon from '@/components/DlIcon.vue'

// 新建实例是一次四个选择的组合：机器、版本、数据目录、名称。一次把所有
// 条件摊开，不做多步向导，也不把「去哪儿装版本」藏起来。

const props = defineProps<{
  visible: boolean
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
const router = useRouter()

const hostId = ref(store.currentHostId)
const versionId = ref<string | undefined>(undefined)
const homeId = ref<string | undefined>(undefined)
const name = ref('')

const host = computed(() => store.hostById(hostId.value))
const versions = computed(() => store.versionsByHost[hostId.value] ?? [])
const homes = computed(() => store.homesByHost[hostId.value] ?? [])
const hostError = computed(
  () =>
    store.versionErrors[hostId.value] || store.homeErrors[hostId.value] || '',
)
const noVersions = computed(() => versions.value.length === 0)
const noHomes = computed(() => homes.value.length === 0)

function uniqueName(base: string): string {
  const existing = new Set(
    store.instancesOfHost(hostId.value).map((i) => i.name),
  )
  if (!existing.has(base)) return base
  let n = 2
  while (existing.has(`${base} ${n}`)) n += 1
  return `${base} ${n}`
}

function reset() {
  hostId.value = props.preset?.hostId ?? store.currentHostId
  if (
    !store.versionsByHost[hostId.value] &&
    !store.versionErrors[hostId.value]
  ) {
    void store.loadHostResources(hostId.value)
  }
  versionId.value = props.preset?.versionId ?? versions.value[0]?.id
  homeId.value = props.preset?.homeId ?? homes.value[0]?.id
  name.value = props.preset
    ? uniqueName(`${props.preset.name} 副本`)
    : uniqueName('新实例')
  if (!props.preset) {
    // 如果默认数据目录还没读出来，等列表到达后再补上。
    window.setTimeout(() => {
      if (!homeId.value) homeId.value = homes.value[0]?.id
    }, 0)
  }
}

watch(
  () => props.visible,
  (v) => {
    if (v) reset()
  },
)

watch(hostId, () => {
  versionId.value = versions.value[0]?.id
  homeId.value = homes.value[0]?.id
  name.value = uniqueName('新实例')
  if (
    !store.versionsByHost[hostId.value] &&
    !store.versionErrors[hostId.value]
  ) {
    void store.loadHostResources(hostId.value)
  }
})

watch(versions, (list) => {
  if (!versionId.value || !list.some((v) => v.id === versionId.value)) {
    versionId.value = list[0]?.id
  }
})

watch(homes, (list) => {
  if (!homeId.value || !list.some((h) => h.id === homeId.value)) {
    homeId.value = list[0]?.id
  }
})

const valid = computed(
  () =>
    !!name.value.trim() &&
    !!versionId.value &&
    !!homeId.value &&
    !noVersions.value &&
    !noHomes.value,
)

const createAction = useAction(
  () =>
    api.createInstance(
      name.value.trim(),
      versionId.value!,
      homeId.value!,
      hostId.value,
    ),
  { success: () => '实例已创建' },
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

function goInstall() {
  void store.selectHost(hostId.value)
  close()
  void router.push('/install')
}

function goData() {
  void store.selectHost(hostId.value)
  close()
  void router.push('/data')
}
</script>

<template>
  <a-modal
    :visible="visible"
    :mask-closable="true"
    width="480px"
    title="新建实例"
    @update:visible="(v) => emit('update:visible', v)"
  >
    <div v-if="hostError" class="notice notice--warn">
      <div class="notice-body">
        <div class="notice-title">读不到这台机器的资源</div>
        <div>{{ hostError }}</div>
        <div style="margin-top: 8px">
          <button
            class="btn btn--quiet btn--sm"
            @click="store.loadHostResources(hostId)"
          >
            重新读取
          </button>
        </div>
      </div>
    </div>

    <template v-else>
      <div v-if="store.hosts.length > 1" class="field">
        <label class="field-label">机器</label>
        <a-select v-model="hostId" style="width: 100%">
          <a-option v-for="h in store.hosts" :key="h.id" :value="h.id">
            {{ h.display_name }}
          </a-option>
        </a-select>
      </div>

      <div v-if="noVersions" class="notice notice--info">
        <div class="notice-body">
          <div class="notice-title">这台机器还没有安装 DSH 版本</div>
          <div>
            先安装一个版本，安装完成后会自动创建它专用的数据目录和第一个实例。
          </div>
          <button
            class="btn btn--primary btn--sm"
            style="margin-top: 10px"
            @click="goInstall"
          >
            去安装
          </button>
        </div>
      </div>

      <template v-else>
        <div class="field">
          <label class="field-label">DSH 版本</label>
          <a-select v-model="versionId" style="width: 100%">
            <a-option v-for="v in versions" :key="v.id" :value="v.id">{{
              v.version
            }}</a-option>
          </a-select>
        </div>

        <div v-if="noHomes" class="notice notice--info">
          <div class="notice-body">
            <div class="notice-title">这台机器还没有数据目录</div>
            <div>实例需要绑定一个 DSH_HOME 来保存会话、凭据和配置。</div>
            <button
              class="btn btn--primary btn--sm"
              style="margin-top: 10px"
              @click="goData"
            >
              去创建数据目录
            </button>
          </div>
        </div>

        <div v-else class="field">
          <label class="field-label">数据目录（DSH_HOME）</label>
          <a-select v-model="homeId" style="width: 100%">
            <a-option v-for="h in homes" :key="h.id" :value="h.id">
              {{ h.name }} — {{ h.path }}
            </a-option>
          </a-select>
          <span class="field-hint"
            >会话、凭据和 Profile 都存在这个目录里。</span
          >
        </div>

        <div class="field">
          <label class="field-label">实例名称</label>
          <input
            v-model="name"
            class="input"
            placeholder="例如：主实例"
            @keydown.enter="onCreate"
          />
        </div>
      </template>
    </template>

    <template #footer>
      <div class="dialog-footer">
        <button class="btn btn--quiet" @click="close">取消</button>
        <button
          v-if="!noVersions && !noHomes"
          class="btn btn--primary"
          :disabled="!valid || !!createAction.busy['*']"
          @click="onCreate"
        >
          创建实例
        </button>
      </div>
    </template>
  </a-modal>
</template>

<style lang="scss" scoped>
.dialog-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>

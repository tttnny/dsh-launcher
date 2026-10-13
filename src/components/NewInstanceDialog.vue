<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
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
   * launch time.
   */
  preset?: { name: string; versionId: string; homeId: string } | null
}>()
const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'created', id: string): void
}>()

const { t } = useI18n()
const store = useLauncherStore()

const name = ref('')
const versionId = ref<string | undefined>(undefined)
const homeId = ref<string | undefined>(undefined)

/** `base`, then `base 2`, `base 3`… — instance names are unique. */
function uniqueName(base: string): string {
  const existing = new Set(store.instances.map((i) => i.name))
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
    name.value = uniqueName(t('instances.copyName', { name: preset.name }))
    versionId.value = preset.versionId
    homeId.value = preset.homeId
    return
  }
  let n = store.instances.length + 1
  const existing = new Set(store.instances.map((i) => i.name))
  while (existing.has(`${t('instances.namePrefix')}${n}`)) n += 1
  name.value = `${t('instances.namePrefix')}${n}`
  versionId.value = store.versions[0]?.id
  homeId.value = store.homes[0]?.id
}

watch(
  () => props.visible,
  (v) => {
    if (v) reset()
  },
)

// A new instance is a fresh binding, so it can only use a version that is
// already installed — installing is the Versions tab's job, not this sheet's.
const noVersions = computed(() => store.versions.length === 0)
const noHomes = computed(() => store.homes.length === 0)

const createAction = useAction(
  () => api.createInstance(name.value.trim(), versionId.value!, homeId.value!),
  { success: () => t('instances.created') },
)
const saving = computed(() => createAction.busy['*'])

const valid = computed(
  () => !!name.value.trim() && !!versionId.value && !!homeId.value && !noVersions.value && !noHomes.value,
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
    <template #title>{{ preset ? t('instances.duplicate') : t('instances.newInstance') }}</template>

    <!-- Nothing installed yet: the sheet cannot do its job, so it says which
         page can, instead of offering three empty selects. -->
    <div v-if="noVersions" class="dl-empty">
      <p class="dl-empty-title">{{ t('instances.noVersionsTitle') }}</p>
      <p class="dl-empty-text">{{ t('instances.noVersionsDesc') }}</p>
      <button class="dl-btn dl-btn--primary" style="margin-top: 6px" @click="close">
        {{ t('common.close') }}
      </button>
    </div>

    <div v-else class="new-instance-body">
      <div class="dl-field">
        <label class="dl-field-label">{{ t('instanceEdit.name') }}</label>
        <input
          v-model="name"
          class="dl-input"
          :placeholder="t('instanceEdit.namePlaceholder')"
          @keydown.enter="onCreate"
        />
      </div>

      <div class="dl-field">
        <label class="dl-field-label">{{ t('instanceEdit.version') }}</label>
        <a-select v-model="versionId" style="width: 100%">
          <a-option v-for="v in store.versions" :key="v.id" :value="v.id">{{ v.version }}</a-option>
        </a-select>
      </div>

      <div class="dl-field">
        <label class="dl-field-label">{{ t('instanceEdit.home') }}</label>
        <a-select v-model="homeId" style="width: 100%">
          <a-option v-for="h in store.homes" :key="h.id" :value="h.id">
            {{ h.name }} — {{ h.path }}
          </a-option>
        </a-select>
        <p class="dl-field-hint">{{ t('instances.homeHint') }}</p>
      </div>

    </div>

    <template v-if="!noVersions" #footer>
      <div class="sheet-footer">
        <button class="dl-btn dl-btn--quiet" @click="close">{{ t('common.cancel') }}</button>
        <button class="dl-btn dl-btn--primary" :disabled="!valid || saving" @click="onCreate">
          {{ t('instances.create') }}
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

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import type { HostInput, HostWithStatus, SshAlias } from '@/api/types'
import { useAction } from '@/composables/useAction'
import DlIcon from '@/components/DlIcon.vue'

// 添加或编辑一台机器。这里只是写一条记录，不拨号；能不能连上由「测试连接」回答。
// 本机是内置的，没有 ssh 目标，只能改名字、数据目录、registry 和额外环境变量。

const props = defineProps<{
  visible: boolean
  host?: HostWithStatus | null
}>()

const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'saved'): void
}>()

interface HostForm {
  display_name: string
  ssh_target: string
  registry: string
  data_dir: string
  extra_env: string
}

function emptyForm(): HostForm {
  return {
    display_name: '',
    ssh_target: '',
    registry: '',
    data_dir: '',
    extra_env: '',
  }
}

const form = ref<HostForm>(emptyForm())
const aliases = ref<SshAlias[]>([])
const aliasesLoaded = ref(false)

const editingBuiltin = computed(() => !!props.host?.builtin)
const isEditing = computed(() => !!props.host)
const title = computed(() =>
  !isEditing.value
    ? '添加远程主机'
    : editingBuiltin.value
      ? '编辑本机'
      : '编辑主机',
)

function reset() {
  aliases.value = []
  aliasesLoaded.value = false
  if (!props.host) {
    form.value = emptyForm()
    return
  }
  form.value = {
    display_name: props.host.display_name,
    ssh_target: props.host.ssh_target,
    registry: props.host.registry ?? '',
    data_dir: props.host.data_dir ?? '',
    extra_env: Object.entries(props.host.extra_env ?? {})
      .map(([key, value]) => key + '=' + value)
      .join('\n'),
  }
}

watch(
  () => props.visible,
  (visible) => {
    if (!visible) return
    reset()
    void loadAliases()
  },
)

async function loadAliases() {
  if (aliasesLoaded.value) return
  aliasesLoaded.value = true
  try {
    aliases.value = await api.listSshAliases()
  } catch {
    aliases.value = []
  }
}

function aliasTarget(alias: SshAlias): string {
  const host = alias.user
    ? alias.user + '@' + (alias.host_name ?? alias.alias)
    : (alias.host_name ?? '')
  return [host, alias.port ? ':' + alias.port : ''].filter(Boolean).join('')
}

function parseEnvLines(text: string): Record<string, string> | null {
  const env: Record<string, string> = {}
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line) continue
    const at = line.indexOf('=')
    if (at <= 0) return null
    env[line.slice(0, at).trim()] = line.slice(at + 1).trim()
  }
  return env
}

const saveAction = useAction(
  (id: string, input: HostInput) =>
    id ? api.updateHost(id, input) : api.createHost(input),
  { success: () => '主机已保存' },
)

const saving = computed(() => saveAction.busy['*'])

function close() {
  emit('update:visible', false)
}

async function save() {
  const value = form.value
  if (!value.display_name.trim()) {
    Message.error('请填写机器的显示名')
    return
  }
  if (!editingBuiltin.value && !value.ssh_target.trim()) {
    Message.error(
      '请填写 ssh 目标：~/.ssh/config 里的别名，或 user@host 这样的字符串',
    )
    return
  }
  const extraEnv = parseEnvLines(value.extra_env)
  if (!extraEnv) {
    Message.error('额外环境变量要写成每行一个 KEY=VALUE')
    return
  }
  const input: HostInput = {
    display_name: value.display_name.trim(),
    ssh_target: editingBuiltin.value ? '' : value.ssh_target.trim(),
    registry: value.registry.trim() || null,
    data_dir: value.data_dir.trim() || null,
    extra_env: extraEnv,
  }
  const saved = await saveAction.run(props.host?.id ?? '', input)
  if (!saved) return
  emit('saved')
  close()
}
</script>

<template>
  <a-modal
    :visible="visible"
    :title="title"
    :ok-text="'保存'"
    :cancel-text="'取消'"
    :ok-loading="saving"
    :mask-closable="!saving"
    unmount-on-close
    @ok="save"
    @cancel="close"
  >
    <div class="field">
      <span class="field-label">显示名</span>
      <input
        v-model="form.display_name"
        class="input"
        placeholder="例如：阿里云服务器"
      />
    </div>

    <div v-if="!editingBuiltin" class="field">
      <span class="field-label">SSH 目标</span>
      <input
        v-model="form.ssh_target"
        class="input input--mono"
        placeholder="zjq_aliyun 或 user@host"
      />
      <div v-if="aliases.length" class="alias-row">
        <span class="field-hint">从 ~/.ssh/config 选择：</span>
        <button
          v-for="alias in aliases"
          :key="alias.alias"
          type="button"
          class="chip"
          :class="{ 'is-active': form.ssh_target === alias.alias }"
          :title="aliasTarget(alias)"
          @click="form.ssh_target = alias.alias"
        >
          {{ alias.alias }}
        </button>
      </div>
      <span class="field-hint"
        >整串原样交给系统 ssh；别名、user@host、user@host -p 2222 都可以。</span
      >
    </div>
    <div v-else class="field">
      <span class="field-hint"
        >本机就是运行启动器的这台 Mac，没有 ssh
        目标；这里改的是它的名字与数据目录。</span
      >
    </div>

    <div class="field">
      <span class="field-label">registry（可选）</span>
      <input
        v-model="form.registry"
        class="input"
        placeholder="留空就用启动器自己的"
      />
      <span class="field-hint">只有这台机器有自己的 registry 时才填。</span>
    </div>

    <div class="field">
      <span class="field-label">数据目录（可选）</span>
      <input
        v-model="form.data_dir"
        class="input input--mono"
        placeholder="留空就用那台机器上的默认目录"
      />
      <span class="field-hint"
        >这台机器上放版本、专属数据目录、日志与运行记录的地方；留空则按那台机器的家目录推。</span
      >
    </div>

    <div class="field">
      <span class="field-label">额外环境变量（可选）</span>
      <textarea
        v-model="form.extra_env"
        class="input input--mono"
        placeholder="每行一个：KEY=VALUE"
        rows="3"
      />
      <span class="field-hint">在这台机器上装版本与跑实例时都注入。</span>
    </div>
  </a-modal>
</template>

<style lang="scss" scoped>
.alias-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
}

.chip {
  height: 24px;
  padding: 0 9px;
  font-size: var(--dl-text-xs);
}
</style>

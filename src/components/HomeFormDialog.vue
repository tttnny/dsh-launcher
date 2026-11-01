<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Message } from '@arco-design/web-vue'
import { api } from '@/api'
import { useAction } from '@/composables/useAction'
import { useLauncherStore } from '@/stores/launcher'

// 新建数据目录：名称加路径。本机可以从磁盘上挑一个目录，远程机器只能手写
// 那台机器上的路径——这台 Mac 的文件选择器看不到别人的磁盘。

const props = defineProps<{
  visible: boolean
  hostId: string
  hostLabel: string
  remote: boolean
}>()

const emit = defineEmits<{
  (e: 'update:visible', v: boolean): void
  (e: 'created', id: string): void
}>()

const store = useLauncherStore()

const name = ref('')
const path = ref('')

watch(
  () => props.visible,
  (v) => {
    if (v) {
      name.value = ''
      path.value = ''
    }
  },
)

const valid = computed(() => !!name.value.trim() && !!path.value.trim())

const createAction = useAction(
  (hostId: string, n: string, p: string) => api.createHome(n, p, hostId),
  { success: () => '数据目录已创建' },
)

async function pickDir() {
  if (api.isTauri) {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog')
      const dir = await open({ directory: true, multiple: false })
      if (typeof dir === 'string') path.value = dir
    } catch (e) {
      Message.error(String(e))
    }
  } else {
    Message.info('浏览器预览不支持系统目录选择，请手动输入路径')
  }
}

async function onCreate() {
  if (!valid.value) return
  const home = await createAction.run(
    props.hostId,
    name.value.trim(),
    path.value.trim(),
  )
  if (home === undefined) return
  await store.refreshHomes(props.hostId)
  emit('created', home.id)
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
    width="480px"
    title="新建数据目录"
    @update:visible="(v) => emit('update:visible', v)"
  >
    <div class="field">
      <label class="field-label">名称</label>
      <input
        v-model="name"
        class="input"
        placeholder="例如：默认数据目录"
        @keydown.enter="onCreate"
      />
    </div>

    <div class="field">
      <label class="field-label">目录路径</label>
      <div class="path-row">
        <input
          v-model="path"
          class="input input--mono"
          :placeholder="
            remote
              ? '那台机器上的路径，例如 /home/me/.dsh'
              : '例如：/Users/me/.dsh'
          "
          @keydown.enter="onCreate"
        />
        <button v-if="!remote" class="btn btn--quiet" @click="pickDir">
          选择目录
        </button>
      </div>
      <span class="field-hint">
        {{ `会话、凭据和 Profile 都会保存在「${hostLabel}」的这个目录里。` }}
      </span>
    </div>

    <template #footer>
      <div class="dialog-footer">
        <button class="btn btn--quiet" @click="close">取消</button>
        <button
          class="btn btn--primary"
          :disabled="!valid || !!createAction.busy['*']"
          @click="onCreate"
        >
          创建
        </button>
      </div>
    </template>
  </a-modal>
</template>

<style lang="scss" scoped>
.path-row {
  display: flex;
  align-items: center;
  gap: 8px;

  .input {
    flex: 1;
  }
}

.dialog-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>

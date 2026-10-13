import { useRouter } from 'vue-router'
import { useLauncherStore } from '@/stores/launcher'

/** Where the manager was opened from, so "返回实例" can go back there. */
export type PluginOrigin = 'instances' | 'instance-detail'

/** Query key carrying that origin through the plugin page's URL. */
export const PLUGIN_ORIGIN_QUERY = 'from'

/**
 * The plugin manager is the running DSH's own service, so its entry point
 * belongs to an instance that is actually up. This resolves the exact scope
 * the manager needs — the instance's DSH_HOME and the profile it is running —
 * and carries the instance id (plus where the user came from) so the manager
 * can hand the user back to the row they left.
 */
export function usePluginLink() {
  const router = useRouter()
  const store = useLauncherStore()

  /** True when this instance can open the manager at all. */
  function canManagePlugins(instanceId: string): boolean {
    const status = store.statusOf(instanceId)
    return status.state === 'running' && !!status.profile
  }

  function openPlugins(instanceId: string, from: PluginOrigin) {
    const inst = store.instanceById(instanceId)
    const status = store.statusOf(instanceId)
    if (!inst || !canManagePlugins(instanceId) || !status.profile) return
    void router.push({
      name: 'plugins',
      query: {
        homeId: inst.home_id,
        profile: status.profile,
        instanceId,
        [PLUGIN_ORIGIN_QUERY]: from,
      },
    })
  }

  return { canManagePlugins, openPlugins }
}

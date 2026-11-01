import { reactive, watch } from 'vue'
import type { DshInstance } from '@/api/types'
import { api } from '@/api'

// Module-level singleton state: the profile list cache and the per-instance
// selection live once, shared by instance cards, the health log modal, and
// any launcher flow that needs "which profile does this instance run".

/** Profile lists per HOME id (undefined = not loaded yet). */
const profilesByHome: Record<string, string[] | undefined> = reactive({})
/** Selected profile per instance id (undefined = not chosen yet). */
const selectionByInstance: Record<string, string | undefined> = reactive({})
/** In-flight profile fetches per HOME id. */
const loadingByHome: Record<string, boolean> = reactive({})

/**
 * One cache + one fallback policy for instance profiles. "Which profile does
 * this instance run" used to be re-derived five times with three different
 * fallback orders; the chain lives here now:
 * kept selection → last_profile → default_profile → first in list.
 */
export function useProfiles() {
  /** Fetches (and caches) a HOME's profile list; concurrent calls share the
   *  in-flight request. Returns the list, or [] on failure. */
  async function loadForHome(homeId: string): Promise<string[]> {
    const cached = profilesByHome[homeId]
    if (cached) return cached
    if (loadingByHome[homeId]) {
      // Someone else is fetching; wait for it to land in the cache.
      await new Promise<void>((resolve) => {
        const stop = watch(
          () => profilesByHome[homeId],
          () => {
            stop()
            resolve()
          },
        )
      })
      return profilesByHome[homeId] ?? []
    }
    loadingByHome[homeId] = true
    try {
      const list = await api.listProfiles(homeId)
      profilesByHome[homeId] = list
      return list
    } catch {
      return []
    } finally {
      loadingByHome[homeId] = false
    }
  }

  /** Ensures the instance's selection exists and still exists in its HOME's
   *  list, applying the fallback chain; returns the effective profile. */
  async function ensureSelection(
    inst: DshInstance,
  ): Promise<string | undefined> {
    const list = await loadForHome(inst.home_id)
    const kept = selectionByInstance[inst.id]
    if (kept && list.includes(kept)) return kept
    const fallback =
      (inst.last_profile &&
        list.includes(inst.last_profile) &&
        inst.last_profile) ||
      (inst.default_profile &&
        list.includes(inst.default_profile) &&
        inst.default_profile) ||
      list[0] ||
      undefined
    selectionByInstance[inst.id] = fallback
    return fallback
  }

  /** Drops a HOME's cached list (e.g. after the instance switched HOME). */
  function invalidateHome(homeId: string) {
    delete profilesByHome[homeId]
  }

  return {
    profilesByHome,
    selectionByInstance,
    loadingByHome,
    loadForHome,
    ensureSelection,
    invalidateHome,
  }
}

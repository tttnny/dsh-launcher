import { reactive } from 'vue'
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
/** Why one HOME's profile list could not be read; undefined when it could. */
const profilesError: Record<string, string | undefined> = reactive({})

/** The one shared fetch per HOME id, so every caller awaits the same result. */
const requestsByHome: Record<
  string,
  Promise<string[] | undefined> | undefined
> = {}
/** Per-HOME load generation: a cache invalidation bumps it, so a request that
 *  was already in flight when the list changed no longer writes its stale
 *  result back. */
const generations: Record<string, number> = {}

/**
 * One cache + one fallback policy for instance profiles. "Which profile does
 * this instance run" used to be re-derived five times with three different
 * fallback orders; the chain lives here now:
 * kept selection → last_profile → default_profile → first in list.
 */
export function useProfiles() {
  /** Fetches (and caches) a HOME's profile list; concurrent calls share the
   *  in-flight request. Resolves the list, or undefined when the read failed
   *  (the failure is kept in `profilesError[homeId]`, so it is not the same
   *  answer as an empty list). */
  async function loadForHome(homeId: string): Promise<string[] | undefined> {
    const cached = profilesByHome[homeId]
    if (cached) return cached
    const inFlight = requestsByHome[homeId]
    if (inFlight) return inFlight
    loadingByHome[homeId] = true
    const generation = generations[homeId] ?? 0
    const request = api
      .listProfiles(homeId)
      .then((list) => {
        // Only the latest generation may write the cache: a list invalidated
        // while this request was in flight must not be resurrected.
        if ((generations[homeId] ?? 0) === generation) {
          profilesByHome[homeId] = list
          delete profilesError[homeId]
        }
        return list
      })
      .catch((e) => {
        // A read failure is not "this HOME has no profiles": record the reason
        // and hand back undefined so callers can tell the two apart.
        if ((generations[homeId] ?? 0) === generation) {
          profilesError[homeId] = String(e)
        }
        return undefined
      })
      .finally(() => {
        if (requestsByHome[homeId] === request) {
          delete requestsByHome[homeId]
          loadingByHome[homeId] = false
        }
      })
    requestsByHome[homeId] = request
    return request
  }

  /** Ensures the instance's selection exists and still exists in its HOME's
   *  list, applying the fallback chain; returns the effective profile.
   *
   *  A failed read keeps whatever selection the instance already had instead of
   *  running the chain over an empty list — "could not read" must not clear a
   *  valid choice or read as "no profiles". */
  async function ensureSelection(
    inst: DshInstance,
  ): Promise<string | undefined> {
    const list = await loadForHome(inst.home_id)
    const kept = selectionByInstance[inst.id]
    if (!list) return kept
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
    delete profilesError[homeId]
    delete requestsByHome[homeId]
    generations[homeId] = (generations[homeId] ?? 0) + 1
  }

  /** Clears any selection still naming `profile` after it was renamed or
   *  deleted, so the next check picks one that exists. Scope it to the HOME's
   *  instances when the caller knows them. */
  function forgetSelection(profile: string, instanceIds?: string[]) {
    const ids = instanceIds ?? Object.keys(selectionByInstance)
    for (const id of ids) {
      if (selectionByInstance[id] === profile) delete selectionByInstance[id]
    }
  }

  /** Drops everything cached for one HOME — its profile list, the read in
   *  flight, the failure reason and its generation — once the HOME is removed.
   *  A read still in flight keeps its generation, so its result cannot write a
   *  list back for a HOME that no longer exists. */
  function forgetHome(homeId: string) {
    const inFlight = requestsByHome[homeId]
    invalidateHome(homeId)
    delete loadingByHome[homeId]
    if (!inFlight) delete generations[homeId]
  }

  /** Drops one instance's kept selection once the instance is removed. */
  function forgetInstance(instanceId: string) {
    delete selectionByInstance[instanceId]
  }

  return {
    profilesByHome,
    selectionByInstance,
    loadingByHome,
    profilesError,
    loadForHome,
    ensureSelection,
    invalidateHome,
    forgetSelection,
    forgetHome,
    forgetInstance,
  }
}

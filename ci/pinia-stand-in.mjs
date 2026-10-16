// The small piece of pinia the store tests need: `defineStore(id, options)`
// with a state factory, getters and actions — and nothing more, because that is
// all `src/stores/launcher.ts` uses (no `$patch`, `$reset`, `storeToRefs`,
// plugins or subscribers anywhere under `src/`).
//
// It stands in for the real package because the CI job that runs these tests
// installs no dependencies at all. It is resolved only for the bare `pinia`
// specifier, and only inside a process that loaded `store-harness.mjs`; see
// that file's header for the rest of the environment.
//
// Two deliberate differences from pinia:
//
// - `useStore()` builds a **new** store on every call, so a test starts from a
//   clean state with no app and no active pinia. The app's singleton behaviour
//   is not what these tests read.
// - Getters are plain accessors, taken afresh on every read. With state written
//   through directly, that is what pinia's computeds amount to by the next read
//   anyway.

export function defineStore(id, options) {
  function useStore() {
    const store = { ...(options.state?.() ?? {}) }
    for (const [name, getter] of Object.entries(options.getters ?? {})) {
      Object.defineProperty(store, name, {
        enumerable: true,
        get: () => getter.call(store, store),
      })
    }
    for (const [name, action] of Object.entries(options.actions ?? {})) {
      store[name] = action.bind(store)
    }
    return store
  }
  return Object.assign(useStore, { $id: id })
}

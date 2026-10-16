// Load the frontend's store — and the browser mock under it — into Node, so a
// test can drive real store behaviour (host filtering, 当前主机, the fifth
// instance state, the reachability rules) with no webview, no build step and no
// test framework. The frontend has no test runner by design (no vitest, no
// jsdom); this is the seam that keeps a store-level test as cheap as the
// source-level ones beside it.
//
// Usage (see ci/store-status.test.mjs):
//
//   import { loadStore } from './store-harness.mjs'
//   const { useLauncherStore, mockCall } = await loadStore()
//   const store = useLauncherStore()
//   await store.init()
//
// Reach the sources **through** the harness: a static `import '../src/x.ts'` in
// the test file is resolved before any of its code runs, so it would be resolved
// without the hooks below and fail on the first alias or missing extension.
//
// Traps this file exists to absorb — each of them cost an agent a session:
//
// 1. **Bare Node, no `node_modules`.** The CI job that runs these tests checks
//    the repo out and installs nothing, so this may only use Node's own
//    modules. That rules out both a `vite build --ssr` bundle and the real
//    `pinia`: the bare `pinia` specifier is answered by the small stand-in in
//    `pinia-stand-in.mjs`.
// 2. **`localStorage`.** The mock keeps its whole database in localStorage
//    (`dsh-launcher.mock.v1`); Node has none. The stand-in goes in before
//    anything is imported.
// 3. **`window`, and when it has to exist.** `src/api/tauri.ts` decides
//    `isTauri` once, at import time, by looking for `__TAURI_INTERNALS__` on
//    `window`: a stand-in without that key routes every call to the mock — but
//    the stand-in still has to be there for the commands that call
//    `window.open`.
// 4. **Two specifier shapes Node's resolver refuses.** The sources import
//    `@/api/types` (the Vite alias / tsconfig `paths`), they omit extensions
//    (`./types`), and `@/api` names a *directory* that has to reach its
//    `index.ts`. `import type` needs none of that working — types are stripped
//    before anything is resolved — which is why a module whose imports are all
//    types, like `src/instancesBulk.ts`, needs no harness at all.
// 5. **Node's type stripping is the build step.** The `.ts` sources run as they
//    are; nothing here compiles anything.
// 6. **Nothing is per-test.** The mock's task table, profile list and host
//    statuses live in module memory: `localStorage.clear()` drops the stored
//    database (the next call seeds it again) but not those. One test file is one
//    process — keep that in mind when a test mutates the mock, and take a fresh
//    `useLauncherStore()` per test (the stand-in returns a new store each call).
//
// Run: pnpm test:store-status

import { statSync } from 'node:fs'
import { registerHooks } from 'node:module'
import { fileURLToPath } from 'node:url'

/** `src/` — what the alias `@` stands for in vite.config.ts and tsconfig.json. */
const SRC = new URL('../src/', import.meta.url)

/** The one place the bare `pinia` specifier is answered from; see trap 1. */
const PINIA_STAND_IN = new URL('./pinia-stand-in.mjs', import.meta.url).href

/** A file URL for `specifier`, settled the way the bundler settles it. */
function sourceUrl(specifier, parent) {
  const asFile = (url) => {
    try {
      return statSync(fileURLToPath(url)).isFile() ? url.href : null
    } catch {
      return null
    }
  }
  return (
    asFile(new URL(specifier, parent)) ??
    asFile(new URL(`${specifier}.ts`, parent)) ??
    asFile(new URL(`${specifier}/index.ts`, parent)) ??
    new URL(specifier, parent).href
  )
}

let hooksInstalled = false

/** Teaches Node the specifier shapes the sources use. Idempotent. */
function installSourceHooks() {
  if (hooksInstalled) return
  hooksInstalled = true
  registerHooks({
    resolve(specifier, context, nextResolve) {
      if (specifier === 'pinia') return { url: PINIA_STAND_IN, shortCircuit: true }
      if (specifier.startsWith('@/')) {
        return { url: sourceUrl(specifier.slice(2), SRC), shortCircuit: true }
      }
      if (specifier.startsWith('.')) {
        return { url: sourceUrl(specifier, context.parentURL), shortCircuit: true }
      }
      return nextResolve(specifier, context)
    },
  })
}

/**
 * Gives Node the browser globals the frontend reads, then the specifier hooks.
 * Everything must be in place **before** the first import of a source file.
 */
function installEnvironment() {
  if (typeof globalThis.localStorage === 'undefined') {
    const entries = new Map()
    globalThis.localStorage = {
      getItem: (key) => (entries.has(String(key)) ? entries.get(String(key)) : null),
      setItem: (key, value) => void entries.set(String(key), String(value)),
      removeItem: (key) => void entries.delete(String(key)),
      clear: () => entries.clear(),
      key: (index) => [...entries.keys()][index] ?? null,
      get length() {
        return entries.size
      },
    }
  }
  if (typeof globalThis.window === 'undefined') globalThis.window = { open() {} }
  installSourceHooks()
}

/** The mock's own entry point (`mockCall`), as the api seam reaches it. */
export async function loadMock() {
  installEnvironment()
  const { mockCall } = await import('../src/api/mock.ts')
  return { mockCall, localStorage: globalThis.localStorage }
}

/**
 * The store, ready to drive. `mockCall` comes along because the store reads the
 * mock through the api seam: both imports share one module graph, so a call
 * made here is the very call the store's actions make.
 */
export async function loadStore() {
  const mock = await loadMock()
  const { useLauncherStore } = await import('../src/stores/launcher.ts')
  return { ...mock, useLauncherStore }
}

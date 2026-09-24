// API contract test: the public api interface (src/api/index.ts) may only
// invoke commands the mock adapter (src/api/mock.ts) implements. The two
// adapters drift silently otherwise — the Plugins page once threw
// `mock: unknown command` for three commands in a row.
//
// Run: pnpm test:api-contract (plain node --test, no build step).

import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

const root = new URL('..', import.meta.url)

function read(rel) {
  return readFileSync(new URL(rel, root), 'utf8')
}

/** Command names invoked by the public api object. */
function apiCommands() {
  const src = read('src/api/index.ts')
  const names = new Set()
  for (const m of src.matchAll(/call<[^>]*>\('([\w]+)'/g)) names.add(m[1])
  // Multi-line generic form: call<SomeThing[]>\n  ('cmd'
  for (const m of src.matchAll(/call<([^>]*)>\s*\(\s*'([\w]+)'/g)) names.add(m[2])
  return names
}

/** Command names the mock adapter implements (switch case labels). */
function mockCommands() {
  const src = read('src/api/mock.ts')
  const names = new Set()
  for (const m of src.matchAll(/case '([\w]+)':/g)) names.add(m[1])
  return names
}

/** Commands the Rust backend registers (invoke_handler list in lib.rs). */
function backendCommands() {
  const src = read('src-tauri/src/lib.rs')
  const names = new Set()
  for (const m of src.matchAll(/(\w+)::(\w+),/g)) {
    if (m[1] === 'commands' || m[1] === 'tasks' || m[1] === 'plugins' || m[1] === 'runtime' || m[1] === 'icons' || m[1] === 'update') {
      names.add(m[2])
    }
  }
  return names
}

test('every command the api invokes has a mock implementation', () => {
  const api = apiCommands()
  const mock = mockCommands()
  assert.ok(api.size >= 40, `expected the api surface to cover >=40 commands, found ${api.size}`)
  const missing = [...api].filter((c) => !mock.has(c))
  assert.deepEqual(missing, [], `mock adapter is missing: ${missing.join(', ')}`)
})

test('mock does not implement commands outside the api surface', () => {
  const api = apiCommands()
  const mock = mockCommands()
  // Mock-internal delegation target (start_install_version_task delegates to
  // it, mirroring the backend).
  const internal = new Set(['start_create_instance_task'])
  const extra = [...mock].filter((c) => !api.has(c) && !internal.has(c))
  assert.deepEqual(extra, [], `mock implements commands no caller uses: ${extra.join(', ')}`)
})

test('every backend command is either invoked by the api or deliberately absent', () => {
  const api = apiCommands()
  const backend = backendCommands()
  // Deliberately not exposed to the frontend (internal/pending commands).
  const intentional = new Set(['pending_deep_link', 'get_launcher_directory'])
  const unexposed = [...backend].filter((c) => !api.has(c) && !intentional.has(c))
  assert.deepEqual(unexposed, [], `backend commands with no frontend caller: ${unexposed.join(', ')}`)
})

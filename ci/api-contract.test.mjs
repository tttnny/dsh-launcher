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

// ---------------------------------------------------------------------------
// Argument names
// ---------------------------------------------------------------------------
// The third kind of drift, and the quietest: the command name is right and the
// argument key is not. Tauri looks an argument up under one exact name —
// snake_case for a command that declares `rename_all = "snake_case"`, camelCase
// otherwise — and a `Option<T>` argument under a name nobody sent is not an
// error: it is `None`. So `start_install_version_task` called with `hostId`
// instead of `host_id` silently installed onto 本机 while the page named the
// user's server.

/** The argument names the api sends, by command. */
function apiArguments() {
  const src = read('src/api/index.ts')
  const out = new Map()
  // No argument object in this file nests braces or parens, so the literal ends
  // at its first `}`.
  for (const m of src.matchAll(/call<[^<>]*>\s*\(\s*'(\w+)'\s*(?:,\s*(\{[\s\S]*?\})\s*)?\)/g)) {
    const keys = new Set()
    if (m[2]) {
      for (const part of m[2].slice(1, -1).split(',')) {
        const key = part.split(':')[0].trim()
        if (key) keys.add(key)
      }
    }
    out.set(m[1], keys)
  }
  return out
}

/** Arguments Tauri supplies itself; a caller never sends them. */
const INJECTED = new Set(['app', '_app', 'state', 'window', 'webview', 'webview_window'])

function toCamel(name) {
  return name.replace(/_([a-z])/g, (_, c) => c.toUpperCase())
}

/** Splits a Rust parameter list on its top-level commas. */
function parametersOf(text) {
  const parts = []
  let depth = 0
  let current = ''
  for (const char of text) {
    if ('<([{'.includes(char)) depth += 1
    if (')]}>'.includes(char) && depth > 0) depth -= 1
    if (char === ',' && depth === 0) {
      parts.push(current)
      current = ''
      continue
    }
    current += char
  }
  parts.push(current)
  return parts
}

/** The argument names each backend command reads, by command. */
function backendArguments() {
  const out = new Map()
  // Every file the handler list draws commands from, `lib.rs` included (one
  // command lives there).
  for (const file of ['lib', 'commands', 'tasks', 'plugins', 'runtime', 'icons', 'update']) {
    const src = read(file === 'lib' ? 'src-tauri/src/lib.rs' : `src-tauri/src/${file}.rs`)
    const command =
      /#\[tauri::command([^\]]*)\]\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)\s*\(([\s\S]*?)\)\s*(?:->|\{)/g
    for (const m of src.matchAll(command)) {
      const snake = /rename_all\s*=\s*"snake_case"/.test(m[1])
      // key -> whether the command cannot be called without it.
      const keys = new Map()
      for (const param of parametersOf(m[3])) {
        const named = /^([a-z_]\w*)\s*:\s*([\s\S]+)$/.exec(param.trim())
        if (!named || INJECTED.has(named[1])) continue
        keys.set(snake ? named[1] : toCamel(named[1]), !/^Option\s*</.test(named[2].trim()))
      }
      out.set(m[2], keys)
    }
  }
  return out
}

test('every argument the api sends is spelled the way the backend reads it', () => {
  const api = apiArguments()
  const backend = backendArguments()
  assert.ok(api.size >= 40, `expected the api surface to cover >=40 commands, found ${api.size}`)
  assert.ok(backend.size >= 40, `expected >=40 backend commands, found ${backend.size}`)

  // A command this file cannot find is a hole in the check, not a passing case.
  const unread = [...api].filter(([name]) => !backend.has(name)).map(([name]) => name)
  assert.deepEqual(unread, [], `api commands with no parsed backend signature: ${unread.join(', ')}`)

  const wrong = []
  const missing = []
  for (const [name, keys] of api) {
    const expected = backend.get(name)
    for (const key of keys) {
      if (!expected.has(key)) {
        wrong.push(`${name}: sends '${key}', reads ${[...expected.keys()].join(', ') || '(nothing)'}`)
      }
    }
    for (const [key, required] of expected) {
      if (required && !keys.has(key)) missing.push(`${name}: '${key}' is required and never sent`)
    }
  }
  assert.deepEqual(wrong, [], wrong.join('\n'))
  assert.deepEqual(missing, [], missing.join('\n'))
})

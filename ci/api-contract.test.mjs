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
  for (const m of src.matchAll(/call<([^>]*)>\s*\(\s*'([\w]+)'/g))
    names.add(m[2])
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
    if (
      m[1] === 'commands' ||
      m[1] === 'tasks' ||
      m[1] === 'plugins' ||
      m[1] === 'runtime' ||
      m[1] === 'icons' ||
      m[1] === 'update'
    ) {
      names.add(m[2])
    }
  }
  return names
}

test('every command the api invokes has a mock implementation', () => {
  const api = apiCommands()
  const mock = mockCommands()
  assert.ok(
    api.size >= 40,
    `expected the api surface to cover >=40 commands, found ${api.size}`,
  )
  const missing = [...api].filter((c) => !mock.has(c))
  assert.deepEqual(
    missing,
    [],
    `mock adapter is missing: ${missing.join(', ')}`,
  )
})

test('mock does not implement commands outside the api surface', () => {
  const api = apiCommands()
  const mock = mockCommands()
  // Mock-internal delegation target (start_install_version_task delegates to
  // it, mirroring the backend).
  const internal = new Set(['start_create_instance_task'])
  const extra = [...mock].filter((c) => !api.has(c) && !internal.has(c))
  assert.deepEqual(
    extra,
    [],
    `mock implements commands no caller uses: ${extra.join(', ')}`,
  )
})

test('every backend command is either invoked by the api or deliberately absent', () => {
  const api = apiCommands()
  const backend = backendCommands()
  // Deliberately not exposed to the frontend (internal/pending commands).
  const intentional = new Set(['pending_deep_link', 'get_launcher_directory'])
  const unexposed = [...backend].filter(
    (c) => !api.has(c) && !intentional.has(c),
  )
  assert.deepEqual(
    unexposed,
    [],
    `backend commands with no frontend caller: ${unexposed.join(', ')}`,
  )
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
  for (const m of src.matchAll(
    /call<[^<>]*>\s*\(\s*'(\w+)'\s*(?:,\s*(\{[\s\S]*?\})\s*)?\)/g,
  )) {
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
const INJECTED = new Set([
  'app',
  '_app',
  'state',
  'window',
  'webview',
  'webview_window',
])

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

/** Every `#[tauri::command]` declaration in the files the handler list draws from. */
function* rustCommands() {
  const command =
    /#\[tauri::command([^\]]*)\]\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)\s*\(([\s\S]*?)\)\s*(?:->|\{)/g
  // `lib.rs` included: one command lives there.
  for (const file of [
    'lib',
    'commands',
    'tasks',
    'plugins',
    'runtime',
    'icons',
    'update',
  ]) {
    const src = read(
      file === 'lib' ? 'src-tauri/src/lib.rs' : `src-tauri/src/${file}.rs`,
    )
    for (const m of src.matchAll(command)) {
      yield { name: m[2], attribute: m[1], params: m[3] }
    }
  }
}

/** Whether one declaration reads its arguments under the underscore spelling. */
function readsSnakeCase(attribute) {
  return /rename_all\s*=\s*"snake_case"/.test(attribute)
}

/** The argument names each backend command reads, by command. */
function backendArguments() {
  const out = new Map()
  for (const { name, attribute, params } of rustCommands()) {
    const snake = readsSnakeCase(attribute)
    // key -> whether the command cannot be called without it.
    const keys = new Map()
    for (const param of parametersOf(params)) {
      const named = /^([a-z_]\w*)\s*:\s*([\s\S]+)$/.exec(param.trim())
      if (!named || INJECTED.has(named[1])) continue
      keys.set(
        snake ? named[1] : toCamel(named[1]),
        !/^Option\s*</.test(named[2].trim()),
      )
    }
    out.set(name, keys)
  }
  return out
}

test('every argument the api sends is spelled the way the backend reads it', () => {
  const api = apiArguments()
  const backend = backendArguments()
  assert.ok(
    api.size >= 40,
    `expected the api surface to cover >=40 commands, found ${api.size}`,
  )
  assert.ok(
    backend.size >= 40,
    `expected >=40 backend commands, found ${backend.size}`,
  )

  // A command this file cannot find is a hole in the check, not a passing case.
  const unread = [...api]
    .filter(([name]) => !backend.has(name))
    .map(([name]) => name)
  assert.deepEqual(
    unread,
    [],
    `api commands with no parsed backend signature: ${unread.join(', ')}`,
  )

  const wrong = []
  const missing = []
  for (const [name, keys] of api) {
    const expected = backend.get(name)
    for (const key of keys) {
      if (!expected.has(key)) {
        wrong.push(
          `${name}: sends '${key}', reads ${[...expected.keys()].join(', ') || '(nothing)'}`,
        )
      }
    }
    for (const [key, required] of expected) {
      if (required && !keys.has(key))
        missing.push(`${name}: '${key}' is required and never sent`)
    }
  }
  assert.deepEqual(wrong, [], wrong.join('\n'))
  assert.deepEqual(missing, [], missing.join('\n'))
})

// ---------------------------------------------------------------------------
// The mock reads the same keys
// ---------------------------------------------------------------------------
// The check above compares the api with the backend. The mock adapter is the
// third reader, and the one a human actually watches, but a key it accepts under
// either spelling can never disagree with the api — so the whole class above was
// invisible in the browser preview. The mock resolves an argument through one
// table now (`SNAKE_CASE_COMMANDS` + `argOf` in mock.ts) and reads the rest as
// literal keys; both are checked here against the Rust sources.

/** The commands the mock declares as reading the underscore spelling. */
function mockSnakeCommands() {
  const src = read('src/api/mock.ts')
  const block = /const SNAKE_CASE_COMMANDS[\s\S]*?\[([\s\S]*?)\]/.exec(src)
  if (!block) throw new Error('mock.ts no longer declares SNAKE_CASE_COMMANDS')
  return new Set([...block[1].matchAll(/'(\w+)'/g)].map((m) => m[1]))
}

/** The commands whose Rust attribute says `rename_all = "snake_case"`. */
function backendSnakeCommands() {
  const names = new Set()
  for (const { name, attribute } of rustCommands()) {
    if (readsSnakeCase(attribute)) names.add(name)
  }
  return names
}

test('the mock reads underscore keys for exactly the commands the backend does', () => {
  const mock = mockSnakeCommands()
  const backend = backendSnakeCommands()
  assert.ok(
    mock.size >= 5,
    `expected the mock's table to name >=5 commands, found ${mock.size}`,
  )
  assert.ok(
    backend.size >= 5,
    `expected >=5 Rust commands with rename_all, found ${backend.size}`,
  )
  assert.deepEqual(
    [...mock].sort(),
    [...backend].sort(),
    'the mock table and the backend disagree about which commands read the underscore spelling',
  )
})

/** camelCase -> the underscore spelling one of those commands reads. */
function toSnake(name) {
  return name.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`)
}

/** Each `case '<cmd>':` body of the mock adapter, by command. */
function mockCaseBodies() {
  const src = read('src/api/mock.ts')
  const labels = [...src.matchAll(/case '(\w+)':|default:/g)]
  const out = new Map()
  labels.forEach((m, i) => {
    if (!m[1]) return
    const end = i + 1 < labels.length ? labels[i + 1].index : src.length
    out.set(m[1], src.slice(m.index, end))
  })
  return out
}

test('every argument the mock reads is one its command reads', () => {
  const backend = backendArguments()
  const snake = backendSnakeCommands()
  const bodies = mockCaseBodies()
  assert.ok(
    bodies.size >= 40,
    `expected the mock to implement >=40 commands, found ${bodies.size}`,
  )

  // Reached only through another command, never through Tauri: its keys are the
  // Rust function's own parameter names, so the attribute rule does not apply.
  const internal = new Set(['start_create_instance_task'])

  const wrong = []
  for (const [name, body] of bodies) {
    if (!backend.has(name)) {
      if (!internal.has(name))
        wrong.push(`${name}: not a command the backend registers`)
      continue
    }
    const keys = backend.get(name)
    // What the body spells literally, plus what it hands to `argOf` — the two
    // ways an argument is read there.
    const reads = [
      ...[...body.matchAll(/args\?\.(\w+)/g)].map((m) => m[1]),
      ...[...body.matchAll(/argOf\(\s*args\s*,\s*cmd\s*,\s*'(\w+)'\s*\)/g)].map(
        (m) => (snake.has(name) ? toSnake(m[1]) : m[1]),
      ),
    ]
    for (const key of new Set(reads)) {
      if (!keys.has(key)) {
        wrong.push(
          `${name}: reads '${key}', backend reads ${[...keys.keys()].join(', ') || '(nothing)'}`,
        )
      }
    }
  }
  assert.deepEqual(wrong, [], wrong.join('\n'))
})

test('no mock argument read falls back to a second spelling', () => {
  const src = read('src/api/mock.ts')
  const fallbacks = [
    ...src.matchAll(/args\?\.(\w+)\s*\?\?\s*args\?\.(\w+)/g),
  ].map((m) => `${m[1]} ?? ${m[2]}`)
  assert.deepEqual(
    fallbacks,
    [],
    `a command reads one name per argument, so one of these keys is always the wrong spelling: ${fallbacks.join('; ')}`,
  )
})

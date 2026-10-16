// The gate list is a single source of truth again — that is the whole point of
// `ci/verify.sh`. The failure mode this file guards against is the one the
// script replaced: a second copy of the list, in the script, in the workflow,
// or in package.json, that nobody notices has fallen behind.
//
// Three copies would each rot differently, so each is refused here by shape
// rather than by content:
//
// - `ci/verify.sh` may name no test file: it discovers `ci/*.test.mjs` by glob,
//   and the moment one suite is spelled out the next one can be added, pass,
//   and never run.
// - `.github/workflows/ci.yml` may not carry gates of its own (`cargo fmt`,
//   `cargo clippy`, `cargo test`, `node --test`, `node ci/check-versions.mjs`,
//   `pnpm build`). Both jobs run `ci/verify.sh` instead; a `continue-on-error`
//   format check is how the drift this repo just ended stayed invisible.
// - `package.json` has to expose that script to whoever is told «提交前跑
//   pnpm verify» — a pointer in the docs is worthless if the script is gone.
//
// Run: pnpm test:gate-list (ci/verify.sh runs it with the rest).
import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

const root = new URL('..', import.meta.url)
const read = (path) => readFileSync(new URL(path, root), 'utf8')

const verifySh = read('ci/verify.sh')
const workflow = read('.github/workflows/ci.yml')
const pkg = JSON.parse(read('package.json'))

test('verify.sh discovers the suites by glob, never from a written list', () => {
  assert.match(
    verifySh,
    /ci\/\*\.test\.mjs/,
    'verify.sh no longer globs ci/*.test.mjs',
  )
  assert.deepEqual(
    verifySh.match(/[\w-]+\.test\.mjs/g) ?? [],
    [],
    'verify.sh names a suite by hand; new suites would silently stop being run',
  )
})

test('the workflow runs the gates through verify.sh, not from its own list', () => {
  assert.match(
    workflow,
    /^ *run: pnpm verify$/m,
    'the macOS job no longer runs pnpm verify',
  )
  assert.match(
    workflow,
    /^ *run: \.\/ci\/verify\.sh --source-only$/m,
    'the Linux job no longer runs the dependency-free mode of verify.sh',
  )

  const ownCopy = workflow
    .split('\n')
    .filter((line) => !line.trimStart().startsWith('#'))
    .filter((line) =>
      /cargo (fmt|clippy|test)\b|node --test\b|node ci\/check-versions\.mjs|pnpm build\b/.test(
        line,
      ),
    )
  assert.deepEqual(
    ownCopy,
    [],
    `the workflow carries its own gate list again:\n${ownCopy.join('\n')}`,
  )
})

test('package.json exposes both modes of verify.sh', () => {
  assert.match(
    pkg.scripts?.verify ?? '',
    /ci\/verify\.sh/,
    'no `pnpm verify` script',
  )
  assert.match(
    pkg.scripts?.['verify:quick'] ?? '',
    /ci\/verify\.sh --quick/,
    'no `pnpm verify:quick` script',
  )
  for (const mode of ['--quick', '--source-only']) {
    assert.ok(verifySh.includes(mode), `verify.sh no longer documents ${mode}`)
  }
})

// Setup-page guidance contract.
//
// The two install-guidance blocks must be independent: each missing tool gets
// its own block. They were once mutually exclusive (the pnpm block's condition
// was `nodeOk && !pnpmOk`), which meant a machine lacking both was told only
// how to install Node — the user would install Node, assume they were ready,
// and hit the missing-pnpm error on their first version install. pnpm is a hard
// prerequisite for installing a version or managing plugins, so that omission
// sent them into a failure the page existed to prevent.
//
// This is a source-level guard, not a render test: the frontend has no test
// runner (no vitest/jsdom), and the rendered behaviour is verified in the real
// app. Its job is to stop the mutual exclusion from creeping back.
//
// Run: node --test ci/setup-guidance.test.mjs

import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

const root = new URL('..', import.meta.url)

function read(rel) {
  return readFileSync(new URL(rel, root), 'utf8')
}

/** The `v-if` condition on the guide block whose heading is `i18nKey`. */
function guideBlockCondition(src, i18nKey) {
  const re = new RegExp(
    `<div v-if="([^"]*)" class="guide-block">\\s*<h4>\\{\\{\\s*t\\('${i18nKey}'\\)\\s*\\}\\}</h4>`,
  )
  const m = re.exec(src)
  assert.ok(m, `no guide block found for ${i18nKey}`);
  return m[1]
}

test('each missing tool has its own install-guidance block', () => {
  const src = read('src/views/Setup.vue')

  guideBlockCondition(src, 'setup.installNode')
  guideBlockCondition(src, 'setup.installPnpm')
})

test('the pnpm guidance does not depend on node being present', () => {
  const src = read('src/views/Setup.vue')
  const condition = guideBlockCondition(src, 'setup.installPnpm')

  // The condition may only test pnpm itself. Anything mentioning node would
  // hide the pnpm command from exactly the machine that needs both.
  assert.ok(
    !/nodeOk/.test(condition),
    `pnpm guidance must not be gated on node (condition: ${condition})`,
  )
  assert.ok(
    /pnpmOk/.test(condition),
    `pnpm guidance should be tied to pnpm's own state (condition: ${condition})`,
  )
})

test('a machine missing both tools is shown both commands', () => {
  const src = read('src/views/Setup.vue')
  const nodeCondition = guideBlockCondition(src, 'setup.installNode')
  const pnpmCondition = guideBlockCondition(src, 'setup.installPnpm')

  // Evaluate both conditions for the "nothing installed" state, which is the
  // state of a fresh machine. Both must be true.
  const fn = new Function('nodeOk', 'pnpmOk', `return [${nodeCondition}, ${pnpmCondition}]`)
  assert.deepEqual(
    fn(false, false),
    [true, true],
    'with node and pnpm both missing, both guidance blocks must render',
  )
  // And each block disappears on its own once its tool arrives.
  assert.deepEqual(fn(true, false), [false, true], 'node present: only pnpm guidance')
  assert.deepEqual(fn(false, true), [true, false], 'pnpm present: only node guidance')
})

// The board's 全部启动 / 全部停止 act on the rows in view, split by what each
// button would do to them. That rule lives in `src/instancesBulk.ts` as a plain
// function of (visible rows, status lookup) precisely so it can be driven here:
// no webview, no store, no DOM. Node strips the types on import, so this runs
// under plain `node --test` with no build step.
//
// Run: pnpm test:instances-bulk

import test from 'node:test'
import assert from 'node:assert/strict'

const { bulkScope } = await import('../src/instancesBulk.ts')

function instance(id) {
  return {
    id,
    name: id,
    version_id: 'v1',
    home_id: 'h-default',
    host_id: 'host-local',
  }
}

/** The status lookup the store hands in, as a table of what it knows. */
function statusOf(byId) {
  return (id) => ({ id, state: byId[id] ?? 'stopped' })
}

test('a stopped row is what 全部启动 acts on', () => {
  const scope = bulkScope([instance('i-1')], statusOf({ 'i-1': 'stopped' }))

  assert.deepEqual(scope.start.map((i) => i.id), ['i-1'])
  assert.deepEqual(scope.stop, [])
  assert.deepEqual(scope.unreachable, [])
})

test('a running row is what 全部停止 acts on', () => {
  const scope = bulkScope([instance('i-1')], statusOf({ 'i-1': 'running' }))

  assert.deepEqual(scope.stop.map((i) => i.id), ['i-1'])
  assert.deepEqual(scope.start, [])
})

test('a row on a machine the launcher cannot reach is left alone and reported', () => {
  const scope = bulkScope([instance('i-1')], statusOf({ 'i-1': 'unreachable' }))

  assert.deepEqual(
    scope.unreachable.map((i) => i.id),
    ['i-1'],
    '不可达不是「已停止」的一种：启动器说不出那台机器上是不是已经在跑',
  )
  assert.deepEqual(scope.start, [], '没有可启动的行')
  assert.deepEqual(scope.stop, [], '也没有可停止的行')
})

test('a row that is still starting is left to finish', () => {
  const scope = bulkScope([instance('i-1')], statusOf({ 'i-1': 'starting' }))

  assert.deepEqual(scope.start, [], '正在启动的不要再启动一次')
  assert.deepEqual(scope.stop, [])
  assert.deepEqual(scope.unreachable, [])
})

test('the scope is the rows it was handed, and no others', () => {
  const rows = [instance('i-1'), instance('i-2'), instance('i-3')]
  const scope = bulkScope(rows, statusOf({ 'i-2': 'running', 'i-3': 'unreachable' }))

  assert.deepEqual(scope.start.map((i) => i.id), ['i-1'])
  assert.deepEqual(scope.stop.map((i) => i.id), ['i-2'])
  assert.deepEqual(scope.unreachable.map((i) => i.id), ['i-3'])
  assert.equal(
    scope.start.length + scope.stop.length + scope.unreachable.length,
    rows.length,
    '每一行都落在一处：筛选、计数与两个按钮读的是同一份结果',
  )
})

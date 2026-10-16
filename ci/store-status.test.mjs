// 「主机不可达」 is the fifth instance state, and the one that must never be
// smoothed over into 「已停止」: 已停止 is what puts a 启动 button in front of a
// user, and pressing it on a machine the launcher cannot see is how a second
// DSH ends up running on a server (ADR-0007). Only the store knows which state
// reaches the board, so the rule is tested here, against the browser mock —
// whose remote sample machine starts out 不可达 — through `ci/store-harness.mjs`.
//
// Run: pnpm test:store-status

import test from 'node:test'
import assert from 'node:assert/strict'
// Importing the harness installs nothing by itself: the globals and the
// specifier hooks go in when it is called, before the first source is imported.
import { loadStore } from './store-harness.mjs'

const { useLauncherStore, mockCall } = await loadStore()

/** An initialized store and the two rows these tests look at. */
async function board() {
  const store = useLauncherStore()
  await store.init()
  const remoteHost = store.hosts.find((h) => !h.builtin)
  const remote = store.instances.find((i) => i.host_id === remoteHost.id)
  const local = store.instances.find((i) => i.host_id !== remoteHost.id)
  return { store, remoteHost, remote, local }
}

test('a row on an unreachable machine is 不可达 in the store, not an absent 已停止', async () => {
  const { store, remote } = await board()

  assert.equal(
    store.statusOf(remote.id).state,
    'unreachable',
    '那台机器没答话，启动器说不出它上面是不是已经在跑',
  )
})

test('a row whose machine answered 「已停止」 is absent from the table', async () => {
  const { store, local } = await board()

  assert.equal(
    local.id in store.statusById,
    false,
    '缺席就是「已停止」的读法：后端不会让远端实例走到这里来',
  )
  assert.equal(store.statusOf(local.id).state, 'stopped')
})

test('the retry on an unreachable row comes back as 不可达 and stays in the table', async () => {
  const { store, remote } = await board()

  // 「重试」 is `api.refreshInstanceStatus(id)`; the mock emits the same answer
  // an unreachable machine gives, and the store's status listener is live.
  await mockCall('refresh_instance_status', { id: remote.id })

  assert.equal(
    remote.id in store.statusById,
    true,
    '不可达是一份答案（那台机器没回应），不是「没有状态」',
  )
  assert.equal(
    store.statusOf(remote.id).state,
    'unreachable',
    '重试之后仍不能读成「已停止」——那是让用户在服务器上点「启动」的那句话',
  )
})

test('a machine that cannot be reached keeps its DSH_HOME records but no version list', async () => {
  const { store, remoteHost } = await board()

  await store.loadHostResources(remoteHost.id)

  assert.ok(
    store.homesByHost[remoteHost.id]?.length > 0,
    'DSH_HOME 是启动器自己的记录，机器连不上也留在页面上',
  )
  assert.equal(
    store.versionsByHost[remoteHost.id],
    undefined,
    '「装了哪些」只有那台机器说得出来，它没答就不给清单',
  )
  assert.ok(
    store.versionErrors[remoteHost.id],
    '失败要留下原因，页面靠它说清楚为什么这里没有版本',
  )
})

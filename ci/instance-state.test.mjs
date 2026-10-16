// 一个实例状态活在三个地方（契约里的「四处」第四处是 store，见文件末尾）。
//
// AGENTS.md 把这件事写成了一条契约：往 `InstanceState` 加一个值，要四处一起改——
// `src/api/types.ts` 的联合、两边 locale 的 `home.status.*`、`src/styles/index.scss`
// 里 `.dl-status` 的 `is-*` 变体、以及 `src/stores/launcher.ts` 的 `statusOf`。
// 少改一处，这个状态就渲染成别的东西；而「主机不可达」渲染成「已停止」，是把用户送到
// 服务器上去点「启动」的那一下（ADR-0007）。
//
// 这个文件把「一起改」变成一条会失败的检查。成员从联合本身读出来，不写死名单：下一个
// 加进联合的状态自动进入检查范围，不需要谁记得回来补一行。
//
// 磁盘上的拼法只有一种：联合的值就是序列化出去的那个名字，i18n 的键就是它，CSS 的类名
// 由 `src/components/InstanceRow.vue` 与 `src/views/InstanceDetail.vue` 里的
// `:class="\`is-${status.state}\`"` 拼出来——三处用的是同一个字符串，所以这里按成员
// 原样比对，没有第二种拼法要推。
//
// store 那一路为什么不逐成员断言：`statusOf` 是一句
// `statusById[id] ?? { state: 'stopped' }`，`init` 里的 `applyStatus` 只按名字判断
// 「行离开表格」的那两个状态（`stopped` / `exited`），其余状态是通用地透传的。所以
// 「每个成员都被 store 提到」对 running / starting / unreachable **是假的**，照着写出来
// 的只会是一句永远为真的话（或者一句逼着人往 store 里塞死名单的话）。能查、而且是真事的
// 是那一句替换值的身份：store 对一个它没有记录的 id 给出的状态，必须是联合里的成员——
// 联合里改了名而 store 没跟上，这里就会红。至于「不可达不能被读成已停止」，
// `ci/store-status.test.mjs` 已经按可观察的行为钉住了。
//
// Run: node --test ci/instance-state.test.mjs

import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { loadStore } from './store-harness.mjs'

const root = new URL('..', import.meta.url)

function read(rel) {
  return readFileSync(new URL(rel, root), 'utf8')
}

/** 联合里至少该有的值：读出来比这还少，说明这段读取没跟上联合的写法。 */
const MIN_MEMBERS = 5

/** `InstanceState` 的成员——从联合里读，不是抄一份名单在这里。 */
function stateMembers() {
  const src = read('src/api/types.ts')
  // 联合可以写成多行，所以读到下一个空行为止，而不是只读第一行。
  const declaration =
    /export type InstanceState\s*=\s*([^\n]*(?:\n(?![ \t]*\n)[^\n]*)*)/.exec(
      src,
    )
  assert.ok(
    declaration,
    'src/api/types.ts 里找不到 `export type InstanceState = …`',
  )
  const members = [...declaration[1].matchAll(/'([\w-]+)'/g)].map((m) => m[1])
  assert.ok(
    members.length >= MIN_MEMBERS,
    `从 InstanceState 的联合里只读出 ${members.length} 个成员（${members.join(', ')}）：` +
      `这个文件靠读那个联合来枚举状态，读出来少于 ${MIN_MEMBERS} 个，要么是联合换了写法而这段读取没跟上，` +
      '要么是联合真的少了一个状态——两种都要人来看一眼，不能当成「通过了」。',
  )
  return members
}

/** 一份 locale 里 `home.status` 那一节。 */
function statusMessages(locale) {
  return JSON.parse(read(`src/locales/${locale}.json`)).home?.status ?? {}
}

/** `.dl-status { … }` 这一块：起始行号与块内文本。 */
function dlStatusBlock() {
  const src = read('src/styles/index.scss')
  const start = /^\.dl-status\s*\{/m.exec(src)
  assert.ok(start, 'src/styles/index.scss 里找不到 `.dl-status {` 这一块')

  let depth = 0
  let end = src.length
  for (let i = start.index; i < src.length; i += 1) {
    if (src[i] === '{') depth += 1
    else if (src[i] === '}') {
      depth -= 1
      if (depth === 0) {
        end = i
        break
      }
    }
  }
  return {
    line: src.slice(0, start.index).split('\n').length,
    body: src.slice(start.index, end + 1),
  }
}

// 先读一次联合：联合读不出来，下面三条都无从谈起，所以这句在文件加载时就要说话，
// 而不是让三条测试各自「通过」。
const members = stateMembers()

test('both locales have a home.status entry for every instance state', () => {
  const offences = []
  for (const locale of ['zh-CN', 'en-US']) {
    const messages = statusMessages(locale)
    const present = Object.keys(messages)
    const missing = members.filter(
      (m) => typeof messages[m] !== 'string' || messages[m] === '',
    )
    if (missing.length > 0) {
      offences.push(
        `src/locales/${locale}.json 的 home.status 少了 ${missing.join(', ')}；现在有 ${present.join(', ')}`,
      )
    }
  }

  assert.deepEqual(
    offences,
    [],
    `${offences.join('\n')}\n两边都要补上 home.status.<状态> 这一条：缺一个键，界面渲染出来的是` +
      '键名本身，而不是一句人话。',
  )
})

test('every instance state has a .dl-status variant', () => {
  const { line, body } = dlStatusBlock()
  const variants = new Set(
    [...body.matchAll(/&\.is-([\w-]+)\s*[,{]/g)].map((m) => m[1]),
  )

  const missing = members.filter((m) => !variants.has(m))
  assert.deepEqual(
    missing,
    [],
    `src/styles/index.scss:${line} 的 .dl-status 里没有 ${missing.map((m) => `&.is-${m}`).join(', ')}；` +
      `这一块现在有 ${[...variants].join(', ')}。少一个变体，这个状态就落到默认那盏灰灯上——` +
      '状态之间看不出区别，而「主机不可达」和「已停止」看起来一样是要出事的（ADR-0007）。',
  )

  // 反过来也一样：联合里没有的状态还留着变体，说明有人改过名而这里没跟上。
  const stale = [...variants].filter((v) => !members.includes(v))
  assert.deepEqual(
    stale,
    [],
    `src/styles/index.scss:${line} 的 .dl-status 里还有 ${stale.map((v) => `&.is-${v}`).join(', ')}，` +
      `而联合里只有 ${members.join(', ')}：这些变体再也不会有元素用上，删掉或者改成现在的名字。`,
  )
})

test('the state the store substitutes for an instance it has no record of is one the union declares', async () => {
  const lines = read('src/stores/launcher.ts').split('\n')
  const found = lines.findIndex((l) => /statusOf\s*:/.test(l))
  const where =
    found >= 0
      ? `src/stores/launcher.ts:${found + 1}`
      : 'src/stores/launcher.ts'

  const { useLauncherStore } = await loadStore()
  const store = useLauncherStore()
  const state = store.statusOf('no-such-instance').state

  assert.ok(
    members.includes(state),
    `${where} 对一个没有记录的 id 回答「${state}」，而联合里只有 ${members.join(', ')}：` +
      '这个值会直接送去渲染状态灯，它不是一个状态就等于这一行没有状态可显示。' +
      '联合里改了名的话，把 store 这句一起改过来。',
  )
})

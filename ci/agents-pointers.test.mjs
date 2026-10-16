// AGENTS.md 的指针契约。
//
// 这份文件被瘦身过一次：每条规则的来龙去脉推给了 `docs/adr/*.md` 与
// `docs/testing-on-a-real-host.md`，「提交前跑什么」换成了一句 `pnpm verify`。
// 换来的代价是指针成了一种承诺——ADR 一改名、文档一搬家、脚本一改名，AGENTS.md 就
// 开始把人指向不存在的东西，而读它的下一个人（或 agent）只会以为是自己看错了：
// 找不到的解释，会被当成「这里大概是那样吧」照做下去。
//
// 所以四样都当作契约检查：AGENTS.md 点到的每一个 ADR-000N 都在 `docs/adr/` 里有同号
// 的文件；每一个 `docs/...` 路径都在盘上；每一个 `ci/...` 路径也在盘上（它点名的都是
// 门禁与脚本——搬家了、改名了，照它做的人会照着一条不存在的命令做下去）；它点名的每
// 一个 `pnpm <脚本>` 都是 package.json 的 scripts 里的一个。
//
// 两种提法都按它们自己的意思读：
//
// - 号到文件只按「文件名以这个号开头」配对，不写死文件名：ADR 换一个更准的 slug 是它
//   该有的自由，这条检查不该拦着。
// - 带 `*` 的提法（`ci/*.test.mjs`、`ci/release-notes-*.mjs`、`pnpm test:*`）说的是
//   「这一类」，所以只要求至少匹配到一个，而不是要求盘上真有一个名字里带星号的文件。
// - `pnpm` 后面第一个词才是脚本名，后面跟着的是传给它的参数：`pnpm tauri build
//   --bundles dmg` 跑的是 package.json 里的 `tauri`。
// - 命令与路径都写在反引号里，而且命令那一段以 `pnpm ` 开头——「端口与 pnpm store」
//   那样的散文不会被当成一条命令。
//
// Run: node --test ci/agents-pointers.test.mjs

import test from 'node:test'
import assert from 'node:assert/strict'
import { existsSync, readdirSync, readFileSync } from 'node:fs'

const root = new URL('..', import.meta.url)

function read(rel) {
  return readFileSync(new URL(rel, root), 'utf8')
}

/** AGENTS.md 的每一行，带 1 起的行号——失败时要指得出是哪一行说的。 */
function agentsLines() {
  return read('AGENTS.md')
    .split('\n')
    .map((text, index) => ({ line: index + 1, text }))
}

/** `docs/adr/` 里的 .md 文件，按名字排序。 */
function adrFiles() {
  try {
    return readdirSync(new URL('docs/adr', root))
      .filter((name) => name.endsWith('.md'))
      .sort()
  } catch {
    return []
  }
}

/** 「0001–0008」这种给人读的号段，好让失败信息说得出「现在是哪些」。 */
function adrRange(files) {
  const numbers = files.map((name) => /^(\d+)/.exec(name)?.[1]).filter(Boolean)
  if (numbers.length === 0) return '(no numbered ADR files)'
  const first = numbers[0]
  const contiguous = numbers.every((n, i) => Number(n) === Number(first) + i)
  return contiguous
    ? `${first}–${numbers[numbers.length - 1]}`
    : numbers.join(', ')
}

/** 一条通配符提法变成能匹配相对路径的正则：`*` 不吃路径分隔符，`.` 按字面读。 */
function globToRegExp(pattern) {
  const escaped = pattern
    .replace(/[.+?^${}()|[\]\\]/g, '\\$&')
    .replace(/\*/g, '[^/]*')
  return new RegExp(`^${escaped}$`)
}

/** `prefix/` 底下所有东西的相对路径（递归），给通配符提法当候选。 */
function filesUnder(prefix) {
  try {
    return readdirSync(new URL(prefix, root), { recursive: true }).map(
      (name) => `${prefix}/${String(name).split('\\').join('/')}`,
    )
  } catch {
    return []
  }
}

/** 通配符提法落空时，把同目录下同后缀的名字列出来——好让人一眼看到现在叫什么。 */
function nearbyNames(pattern) {
  const dir = pattern.slice(0, pattern.lastIndexOf('/')) || '.'
  const suffix = /\*[^/]*(\.[\w.]+)$/.exec(pattern)?.[1] ?? ''
  return filesUnder(dir)
    .filter((name) => name.endsWith(suffix))
    .join(', ')
}

/**
 * AGENTS.md 里以 `prefix` 开头的路径提法：行号、原文、是不是通配符。
 *
 * 路径都写在反引号里（`` `docs/adr/` ``、`` `ci/release-notes-*.mjs` ``）——反引号是
 * 这份文件区分「一个路径」与「一句散文」的方式，也是这条检查读得准的方式。
 */
function pathMentions(prefix) {
  const mentions = []
  for (const { line, text } of agentsLines()) {
    for (const match of text.matchAll(/`([^`]+)`/g)) {
      const raw = match[1]
      if (raw.startsWith(`${prefix}/`))
        mentions.push({ line, raw, glob: raw.includes('*') })
    }
  }
  return mentions
}

/**
 * AGENTS.md 里的 `pnpm <脚本>` 提法：行号、原文、脚本名、是不是通配符。
 *
 * 脚本名是 pnpm 后面第一个词，再往后的都是传给它的参数——`pnpm tauri build
 * --bundles dmg` 跑的是 package.json 里的 `tauri`。那一段必须以 `pnpm ` 开头，所以
 * 「端口与 pnpm store」那样的散文不会被当成一条命令。
 */
function scriptMentions() {
  const mentions = []
  for (const { line, text } of agentsLines()) {
    for (const match of text.matchAll(/`(pnpm\s+[^`]+)`/g)) {
      const script = /^pnpm\s+(\S+)/.exec(match[1])?.[1]
      if (script)
        mentions.push({
          line,
          raw: match[1],
          script,
          glob: script.includes('*'),
        })
    }
  }
  return mentions
}

test('every ADR AGENTS.md names has a file in docs/adr', () => {
  const files = adrFiles()
  assert.ok(
    files.length > 0,
    'docs/adr/ 里一个 .md 都没有，而 AGENTS.md 的每条契约都指向那里',
  )
  const available = adrRange(files)

  // 一行里的每一处都要查，不只是第一处：同一行可能既写 ADR-0006 §4 又提同一个号。
  const offences = []
  for (const { line, text } of agentsLines()) {
    for (const match of text.matchAll(/ADR-(\d+)/g)) {
      // 号按四位补零去配文件，所以 `ADR-7` 与 `ADR-0007` 指向同一份；消息里照抄
      // AGENTS.md 写的那一串，好让人一眼找到那一行。
      const number = match[1].padStart(4, '0')
      if (!files.some((name) => name.startsWith(number))) {
        offences.push(
          `AGENTS.md:${line} 提到 ADR-${match[1]}；docs/adr/ 里是 ${available}`,
        )
      }
    }
  }

  assert.deepEqual(
    offences,
    [],
    `${offences.join('\n')}\n要么把那个 ADR 放回 docs/adr/（文件名以它的号开头即可，slug 随你），` +
      `要么改掉 AGENTS.md 里那一行的引用。`,
  )
})

for (const [prefix, floor] of [
  ['docs', 3],
  ['ci', 5],
]) {
  test(`every ${prefix}/ path AGENTS.md names exists`, () => {
    const mentions = pathMentions(prefix)
    assert.ok(
      mentions.length >= floor,
      `只从 AGENTS.md 里读出 ${mentions.length} 处 ${prefix}/ 提法（少于 ${floor} 处）：` +
        '要么是这份文件真的不再点路径了（那就把这个下限调下来），要么是这段读取没跟上它的写法——' +
        '后者会让这条检查变成一句永远为真的话。',
    )

    const offences = []
    for (const { line, raw, glob } of mentions) {
      if (glob) {
        // 带星号的提法说的是「这一类」，所以要求至少匹配到一个——而不是要求盘上真有
        // 一个名字里带星号的文件。
        if (!filesUnder(prefix).some((name) => globToRegExp(raw).test(name))) {
          offences.push(
            `AGENTS.md:${line} 提到的 ${raw} 一个都匹配不到；${prefix}/ 里同后缀的是 ${nearbyNames(raw)}`,
          )
        }
        continue
      }
      if (!existsSync(new URL(raw.replace(/\/+$/, ''), root))) {
        offences.push(`AGENTS.md:${line} 提到的 ${raw} 在盘上找不到`)
      }
    }

    assert.deepEqual(
      offences,
      [],
      `${offences.join('\n')}\n那个路径要么搬回来，要么把 AGENTS.md 里那一行改成它现在的名字。`,
    )
  })
}

test('every pnpm script AGENTS.md names is a script in package.json', () => {
  const scripts = JSON.parse(read('package.json')).scripts ?? {}
  const names = Object.keys(scripts)
  const mentions = scriptMentions()
  assert.ok(
    mentions.length >= 8,
    `只从 AGENTS.md 里读出 ${mentions.length} 处 pnpm 提法（${mentions.map((m) => m.raw).join('、')}）：` +
      '要么是这份文件真的不再点名脚本了（那就把这个下限调下来），要么是这段读取没跟上它的写法——' +
      '后者会让这条检查变成一句永远为真的话。',
  )

  const offences = []
  for (const { line, raw, script, glob } of mentions) {
    if (glob) {
      // `pnpm test:*` 说的是「这一类脚本」，所以要求至少有一个——它这一句的形式
      // （只跑某一个门禁用对应的 test:* 脚本）本来就只在 scripts 里成立。
      if (!names.some((name) => globToRegExp(script).test(name))) {
        offences.push(
          `AGENTS.md:${line} 提到的 ${raw} 在 scripts 里一个都匹配不到`,
        )
      }
      continue
    }
    if (typeof scripts[script] !== 'string' || scripts[script].trim() === '') {
      offences.push(`AGENTS.md:${line} 提到的 ${raw}：scripts 里没有 ${script}`)
    }
  }

  assert.deepEqual(
    offences,
    [],
    `${offences.join('\n')}\nscripts 里现在有 ${names.join(', ')}。` +
      '照 AGENTS.md 说的那条命令要真能跑起来：要么把脚本（或定这个名字的文件）加回去，' +
      '要么改掉 AGENTS.md 里那一行。（`pnpm verify` 与 `pnpm verify:quick` 本身由' +
      ' `ci/gate-list.test.mjs` 盯着，这里管的是 AGENTS.md 有没有指着它们。）',
  )
})

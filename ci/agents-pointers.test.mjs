// 文档指针的契约：`AGENTS.md` 与它指向的规则文档。
//
// 这两处被瘦身过一次：每条规则的来龙去脉推给了 `docs/adr/*.md` 与
// `docs/testing-on-a-real-host.md`，「提交前跑什么」换成了一句 `pnpm verify`。
// 换来的代价是指针成了一种承诺——ADR 一改名、文档一搬家、脚本一改名，AGENTS.md 就
// 开始把人指向不存在的东西，而读它的下一个人（或 agent）只会以为是自己看错了：
// 找不到的解释，会被当成「这里大概是那样吧」照做下去。
//
// 所以规则文档与 AGENTS.md 一起受检，四样都当作契约：点到每一个 ADR-000N 都在
// `docs/adr/` 里有同号的文件；每一个 `docs/...` 路径都在盘上；每一个 `ci/...` 路径
// 也在盘上（它点名的都是门禁与脚本——搬家了、改名了，照它做的人会照着一条不存在的
// 命令做下去）；它点名的每一个 `pnpm <脚本>` 都是 package.json 的 scripts 里的一个。
// `docs/agents/rules/` 下的每一份都进这份名单，所以新拆一份出来也自动被查。
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
// 规则文档里写的路径一律是仓库根起算的相对路径（与 AGENTS.md 同一套写法），所以读出来
// 的提法直接拿去对盘上的路径即可。
//
// 下限只做一件事：盯住「这份读取还读得到东西」。所以它们量的是全部文档的提法总数，
// 而不是某一处的最低配置——某一处少点两个路径是文档该有的自由，两处都读不出东西才是
// 这段正则失灵了。号码大致是实际数量的六成：现在 15 处 docs/、8 处 ci/、19 处 pnpm，
// 单看 AGENTS.md 是 13 / 0 / 6。
//
// Run: node --test ci/agents-pointers.test.mjs

import test from 'node:test'
import assert from 'node:assert/strict'
import { existsSync, readdirSync, readFileSync } from 'node:fs'

const root = new URL('..', import.meta.url)

function read(rel) {
  return readFileSync(new URL(rel, root), 'utf8')
}

/** `docs/agents/rules/` 里的每一份规则文档，按名字排序。 */
function ruleDocs() {
  try {
    return readdirSync(new URL('docs/agents/rules', root))
      .filter((name) => name.endsWith('.md'))
      .sort()
      .map((name) => `docs/agents/rules/${name}`)
  } catch {
    return []
  }
}

/** 受检的文档：AGENTS.md 与它指向的规则文档。 */
function docs() {
  return ['AGENTS.md', ...ruleDocs()]
}

/** 某一份文档的每一行，带 1 起的行号——失败时要指得出是哪一行说的。 */
function docLines(rel) {
  return read(rel)
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
 * 某一份文档里以 `prefix` 开头的路径提法：文档、行号、原文、是不是通配符。
 *
 * 路径都写在反引号里（`` `docs/adr/` ``、`` `ci/release-notes-*.mjs` ``）——反引号是
 * 这份文件区分「一个路径」与「一句散文」的方式，也是这条检查读得准的方式。
 */
function pathMentions(rel, prefix) {
  const mentions = []
  for (const { line, text } of docLines(rel)) {
    for (const match of text.matchAll(/`([^`]+)`/g)) {
      const raw = match[1]
      if (raw.startsWith(`${prefix}/`))
        mentions.push({ doc: rel, line, raw, glob: raw.includes('*') })
    }
  }
  return mentions
}

/**
 * 某一份文档里的 `pnpm <脚本>` 提法：文档、行号、原文、脚本名、是不是通配符。
 *
 * 脚本名是 pnpm 后面第一个词，再往后的都是传给它的参数——`pnpm tauri build
 * --bundles dmg` 跑的是 package.json 里的 `tauri`。那一段必须以 `pnpm ` 开头，所以
 * 「端口与 pnpm store」那样的散文不会被当成一条命令。
 */
function scriptMentions(rel) {
  const mentions = []
  for (const { line, text } of docLines(rel)) {
    for (const match of text.matchAll(/`(pnpm\s+[^`]+)`/g)) {
      const script = /^pnpm\s+(\S+)/.exec(match[1])?.[1]
      if (script)
        mentions.push({
          doc: rel,
          line,
          raw: match[1],
          script,
          glob: script.includes('*'),
        })
    }
  }
  return mentions
}

test('every ADR the docs name has a file in docs/adr', () => {
  const files = adrFiles()
  assert.ok(
    files.length > 0,
    'docs/adr/ 里一个 .md 都没有，而规则文档的每条契约都指向那里',
  )
  const available = adrRange(files)

  // 一行里的每一处都要查，不只是第一处：同一行可能既写 ADR-0006 §4 又提同一个号。
  const offences = []
  for (const doc of docs()) {
    for (const { line, text } of docLines(doc)) {
      for (const match of text.matchAll(/ADR-(\d+)/g)) {
        // 号按四位补零去配文件，所以 `ADR-7` 与 `ADR-0007` 指向同一份；消息里照抄
        // 文档里写的那一串，好让人一眼找到那一行。
        const number = match[1].padStart(4, '0')
        if (!files.some((name) => name.startsWith(number))) {
          offences.push(
            `${doc}:${line} 提到 ADR-${match[1]}；docs/adr/ 里是 ${available}`,
          )
        }
      }
    }
  }

  assert.deepEqual(
    offences,
    [],
    `${offences.join('\n')}\n要么把那个 ADR 放回 docs/adr/（文件名以它的号开头即可，slug 随你），` +
      `要么改掉文档里那一行的引用。`,
  )
})

for (const [prefix, floor] of [
  ['docs', 8],
  ['ci', 3],
]) {
  test(`every ${prefix}/ path the docs name exists`, () => {
    const mentions = docs().flatMap((doc) => pathMentions(doc, prefix))
    assert.ok(
      mentions.length >= floor,
      `全部文档里只读出 ${mentions.length} 处 ${prefix}/ 提法（少于 ${floor} 处）：` +
        '要么是这些文档真的不再点路径了（那就把这个下限调下来），要么是这段读取没跟上它的写法——' +
        '后者会让这条检查变成一句永远为真的话。',
    )

    const offences = []
    for (const doc of docs()) {
      for (const { line, raw, glob } of pathMentions(doc, prefix)) {
        if (glob) {
          // 带星号的提法说的是「这一类」，所以要求至少匹配到一个——而不是要求盘上真有
          // 一个名字里带星号的文件。
          if (
            !filesUnder(prefix).some((name) => globToRegExp(raw).test(name))
          ) {
            offences.push(
              `${doc}:${line} 提到的 ${raw} 一个都匹配不到；${prefix}/ 里同后缀的是 ${nearbyNames(raw)}`,
            )
          }
          continue
        }
        if (!existsSync(new URL(raw.replace(/\/+$/, ''), root))) {
          offences.push(`${doc}:${line} 提到的 ${raw} 在盘上找不到`)
        }
      }
    }

    assert.deepEqual(
      offences,
      [],
      `${offences.join('\n')}\n那个路径要么搬回来，要么把文档里那一行改成它现在的名字。`,
    )
  })
}

test('every pnpm script the docs name is a script in package.json', () => {
  const scripts = JSON.parse(read('package.json')).scripts ?? {}
  const names = Object.keys(scripts)
  const mentions = docs().flatMap((doc) => scriptMentions(doc))
  assert.ok(
    mentions.length >= 8,
    `全部文档里只读出 ${mentions.length} 处 pnpm 提法（${mentions.map((m) => m.raw).join('、')}）：` +
      '要么是这份文件真的不再点名脚本了（那就把这个下限调下来），要么是这段读取没跟上它的写法——' +
      '后者会让这条检查变成一句永远为真的话。',
  )

  const offences = []
  for (const doc of docs()) {
    for (const { line, raw, script, glob } of scriptMentions(doc)) {
      if (glob) {
        // `pnpm test:*` 说的是「这一类脚本」，所以要求至少有一个——它这一句的形式
        // （只跑某一个门禁用对应的 test:* 脚本）本来就只在 scripts 里成立。
        if (!names.some((name) => globToRegExp(script).test(name))) {
          offences.push(
            `${doc}:${line} 提到的 ${raw} 在 scripts 里一个都匹配不到`,
          )
        }
        continue
      }
      if (
        typeof scripts[script] !== 'string' ||
        scripts[script].trim() === ''
      ) {
        offences.push(`${doc}:${line} 提到的 ${raw}：scripts 里没有 ${script}`)
      }
    }
  }

  assert.deepEqual(
    offences,
    [],
    `${offences.join('\n')}\nscripts 里现在有 ${names.join(', ')}。` +
      '照文档说的那条命令要真能跑起来：要么把脚本（或定这个名字的文件）加回去，' +
      '要么改掉文档里那一行。（`pnpm verify` 与 `pnpm verify:quick` 本身由' +
      ' `ci/gate-list.test.mjs` 盯着，这里管的是文档有没有指着它们。）',
  )
})

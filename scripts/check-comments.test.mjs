import test from 'node:test'
import assert from 'node:assert/strict'
import { analyzeSource, scanSource } from './check-comments.mjs'

const NL = String.fromCharCode(10)

function lines(source) {
  return source.split(NL)
}

test('Rust: line comments, doc comments, strings and raw strings', () => {
  const source = [
    '/// 文档注释',
    'pub fn f() {',
    '    let a = "// 不是注释";',
    '    let b = r#"// 不是注释"#;',
    '    let c = b"// 也不是";',
    "    let d = '/';",
    '    // 行内注释',
    '    let e = 1; // 行尾注释',
    '}',
  ].join(NL)
  const result = analyzeSource('demo.rs', source)
  assert.deepEqual(
    result.comments.map((comment) => comment.style),
    ['doc-line', 'line', 'line'],
  )
  assert.deepEqual(
    result.comments.map((comment) => comment.startLine),
    [1, 7, 8],
  )
})

test('Rust: nested block comments and lifetimes', () => {
  const source = [
    "fn f<'a>(x: &'a str) {",
    '    /* 外层 /* 内层 */ 仍然在外层 */',
    '    let y = 1;',
    '}',
  ].join(NL)
  const result = analyzeSource('demo.rs', source)
  assert.equal(result.comments.length, 1)
  assert.equal(result.comments[0].style, 'block')
  assert.equal(result.comments[0].startLine, 2)
  assert.equal(result.comments[0].endLine, 2)
})

test('JS: template literals, regexes and comments inside interpolation', () => {
  const source = [
    'const a = "' + '// 不是注释' + '"',
    'const b = `' + '// 模板里的文字' + '`',
    'const r = /a\\/\\/b/',
    'const c = `x ${1 /* 内部注释 */} y`',
    '// 真正注释',
  ].join(NL)
  const result = analyzeSource('demo.ts', source)
  assert.deepEqual(
    result.comments.map((comment) => comment.text.trim()),
    ['/* 内部注释 */', '// 真正注释'],
  )
})

test('Vue: script, style and template comments', () => {
  const source = [
    '<template>',
    '  <!-- 模板注释 -->',
    '  <div>{{ x }}</div>',
    '</template>',
    '<script setup lang="ts">',
    '// 脚本注释',
    'const x = 1',
    '</script>',
    '<style lang="scss" scoped>',
    '/* 样式注释 */',
    '.a { color: red; }',
    '</style>',
  ].join(NL)
  const result = analyzeSource('Demo.vue', source)
  assert.deepEqual(
    result.comments.map((comment) => comment.style),
    ['html', 'line', 'block'],
  )
  assert.deepEqual(
    result.comments.map((comment) => comment.startLine),
    [2, 6, 10],
  )
})

test('Shell: heredocs, quotes and comments', () => {
  const source = [
    '#!/usr/bin/env bash',
    'echo "# 引号里的井号"',
    "cat <<'TEXT'",
    '# heredoc 里的井号不是注释',
    'TEXT',
    '# 真正的注释',
    'x="${y#prefix}"',
    'TOTAL=$((5 + ${#LIST[@]} + 1))',
    'echo done # 行尾注释',
  ].join(NL)
  const result = analyzeSource('demo.sh', source)
  assert.deepEqual(
    result.comments.map((comment) => comment.text.trim()),
    ['# 真正的注释', '# 行尾注释'],
  )
})

test('Baseline semantics: new comments break the ratchet, deletions pass', async () => {
  const { compareBaseline } = await import('./check-comments.mjs')
  const current = {
    'a.rs': { commentLines: 5 },
    'b.rs': { commentLines: 0 },
    'c.rs': { commentLines: 3 },
  }
  const baseline = { 'a.rs': 5, 'c.rs': 2 }
  const violations = compareBaseline(current, baseline)
  assert.deepEqual(violations, [{ file: 'c.rs', actual: 3, allowed: 2 }])
})

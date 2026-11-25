#!/usr/bin/env node
import { execFileSync } from 'node:child_process'
import { readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { extname, join } from 'node:path'
import { pathToFileURL } from 'node:url'

const NL = String.fromCharCode(10)
const CR = String.fromCharCode(13)
const TAB = String.fromCharCode(9)
const BS = String.fromCharCode(92)
const BT = String.fromCharCode(96)
const SPACE = ' '
const SCAN_ROOTS = [
  'src',
  'src-tauri/src',
  'src-tauri/build.rs',
  'scripts',
  'prettier.config.mjs',
  'vite.config.ts',
]
const SCAN_EXTENSIONS = new Set([
  '.rs',
  '.ts',
  '.vue',
  '.mjs',
  '.js',
  '.sh',
  '.scss',
])
const SKIP_DIRS = new Set(['node_modules', 'target', 'dist', '.git'])
const DEFAULT_BASELINE = 'scripts/comment-baseline.json'
const IDENT_CHARS =
  'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_$'
const JS_REGEX_KEYWORDS = [
  'return',
  'typeof',
  'case',
  'in',
  'of',
  'new',
  'delete',
  'void',
  'throw',
  'do',
  'else',
  'instanceof',
  'yield',
  'await',
]

function isSpace(c) {
  return c === SPACE || c === TAB || c === CR || c === NL
}

function isIdentChar(c) {
  return c !== undefined && IDENT_CHARS.includes(c)
}

function repoRoot() {
  return execFileSync('git', ['rev-parse', '--show-toplevel'], {
    encoding: 'utf8',
  }).trim()
}

export function collectFiles(root) {
  const out = []
  const visit = (path) => {
    let entries
    try {
      entries = readdirSync(join(root, path), { withFileTypes: true })
    } catch {
      return
    }
    for (const entry of entries) {
      const child = join(path, entry.name)
      if (entry.isDirectory()) {
        if (!SKIP_DIRS.has(entry.name)) visit(child)
        continue
      }
      if (entry.isFile() && SCAN_EXTENSIONS.has(extname(entry.name)))
        out.push(child)
    }
  }
  for (const scanRoot of SCAN_ROOTS) {
    if (extname(scanRoot)) {
      out.push(scanRoot)
      continue
    }
    visit(scanRoot)
  }
  return out.sort()
}

export function lineStartsOf(source) {
  const starts = [0]
  for (let i = 0; i < source.length; i++) {
    if (source[i] === NL) starts.push(i + 1)
  }
  return starts
}

export function lineOf(starts, offset) {
  let low = 0
  let high = starts.length - 1
  while (low < high) {
    const mid = (low + high + 1) >> 1
    if (starts[mid] <= offset) low = mid
    else high = mid - 1
  }
  return low + 1
}

function scanRust(source) {
  const comments = []
  let i = 0
  while (i < source.length) {
    const c = source[i]
    if (c === '/') {
      if (source[i + 1] === '/') {
        const doc = source[i + 2] === '/' || source[i + 2] === '!'
        const start = i
        while (i < source.length && source[i] !== NL) i++
        comments.push({ style: doc ? 'doc-line' : 'line', start, end: i })
        continue
      }
      if (source[i + 1] === '*') {
        const doc = source[i + 2] === '*' || source[i + 2] === '!'
        const start = i
        i += 2
        let depth = 1
        while (i < source.length && depth > 0) {
          if (source[i] === '/' && source[i + 1] === '*') {
            depth++
            i += 2
          } else if (source[i] === '*' && source[i + 1] === '/') {
            depth--
            i += 2
          } else i++
        }
        comments.push({ style: doc ? 'doc-block' : 'block', start, end: i })
        continue
      }
      i++
      continue
    }
    if (c === '"') {
      i = skipRustString(source, i)
      continue
    }
    if (c === "'") {
      i = skipRustQuote(source, i)
      continue
    }
    if ((c === 'r' || c === 'b' || c === 'c') && !isIdentChar(source[i - 1])) {
      const end = skipRustPrefixed(source, i)
      if (end > i) {
        i = end
        continue
      }
    }
    i++
  }
  return comments
}

function skipRustString(source, start) {
  let i = start + 1
  while (i < source.length) {
    const c = source[i]
    if (c === BS) {
      i += 2
      continue
    }
    if (c === '"') return i + 1
    i++
  }
  return source.length
}

function skipRustQuote(source, start) {
  let i = start + 1
  if (source[i] === BS) {
    i++
    if (source[i] === 'u' && source[i + 1] === '{') {
      i += 2
      while (i < source.length && source[i] !== '}') i++
      if (i < source.length) i++
    } else {
      i++
    }
    if (source[i] === "'") return i + 1
    return start + 1
  }
  if (source[i] === "'") return i + 1
  const code = source.codePointAt(i)
  if (code !== undefined) {
    const width = code > 0xffff ? 2 : 1
    if (source[i + width] === "'") return i + width + 1
  }
  return start + 1
}

function skipRustPrefixed(source, start) {
  const c = source[start]
  if (c === 'b') {
    if (source[start + 1] === 'r') {
      const hashStart = start + 2
      let j = hashStart
      while (source[j] === '#') j++
      if (source[j] === '"') {
        const close = '"' + source.slice(hashStart, j)
        const end = source.indexOf(close, j + 1)
        return end === -1 ? source.length : end + close.length
      }
      return start
    }
    if (source[start + 1] === '"') return skipRustString(source, start + 1)
    if (source[start + 1] === "'") return skipRustQuote(source, start + 1)
    return start
  }
  if (c === 'r') {
    let j = start + 1
    while (source[j] === '#') j++
    if (source[j] === '"') {
      const close = '"' + source.slice(start + 1, j)
      const end = source.indexOf(close, j + 1)
      return end === -1 ? source.length : end + close.length
    }
    return start
  }
  if (c === 'c' && source[start + 1] === '"')
    return skipRustString(source, start + 1)
  return start
}

function regexAllowed(prevChar, prevWord) {
  if (prevWord) return JS_REGEX_KEYWORDS.includes(prevWord)
  if (!prevChar) return true
  return '([{=,:;!&|?+-*%^~<>'.includes(prevChar)
}

function scanJs(source, start, end) {
  const comments = []
  const state = { i: start, end, depth: 0, prevChar: '', prevWord: '' }
  scanJsRegion(source, state, comments)
  return comments
}

function scanJsRegion(source, state, comments) {
  while (state.i < state.end) {
    const c = source[state.i]
    if (c === '}') {
      if (state.depth === 0) {
        state.i++
        return
      }
      state.depth--
      state.prevChar = '}'
      state.prevWord = ''
      state.i++
      continue
    }
    if (isSpace(c)) {
      state.i++
      continue
    }
    if (c === '/' && source[state.i + 1] === '/') {
      const doc = source[state.i + 2] === '/'
      const start = state.i
      while (state.i < state.end && source[state.i] !== NL) state.i++
      comments.push({ style: doc ? 'doc-line' : 'line', start, end: state.i })
      continue
    }
    if (c === '/' && source[state.i + 1] === '*') {
      const doc = source[state.i + 2] === '*'
      const start = state.i
      state.i += 2
      while (state.i < state.end) {
        if (source[state.i] === '*' && source[state.i + 1] === '/') {
          state.i += 2
          break
        }
        state.i++
      }
      comments.push({ style: doc ? 'doc-block' : 'block', start, end: state.i })
      continue
    }
    if (c === '"' || c === "'") {
      state.i = skipJsQuoted(source, state.i)
      state.prevChar = c
      state.prevWord = ''
      continue
    }
    if (c === BT) {
      state.i = skipJsTemplate(source, state.i, state.end, comments)
      state.prevChar = BT
      state.prevWord = ''
      continue
    }
    if (c === '/' && regexAllowed(state.prevChar, state.prevWord)) {
      const end = skipJsRegex(source, state.i, state.end)
      if (end > state.i) {
        state.i = end
        state.prevChar = 'x'
        state.prevWord = ''
        continue
      }
    }
    if (/[A-Za-z_$]/.test(c)) {
      let end = state.i + 1
      while (end < state.end && isIdentChar(source[end])) end++
      state.prevWord = source.slice(state.i, end)
      state.prevChar = ''
      state.i = end
      continue
    }
    if (c === '{') {
      state.depth++
      state.prevChar = '{'
      state.prevWord = ''
      state.i++
      continue
    }
    state.prevChar = c
    state.prevWord = ''
    state.i++
  }
}

function skipJsQuoted(source, start) {
  const quote = source[start]
  let i = start + 1
  while (i < source.length) {
    const c = source[i]
    if (c === BS) {
      i += 2
      continue
    }
    if (c === quote) return i + 1
    i++
  }
  return source.length
}

function skipJsTemplate(source, start, end, comments) {
  let i = start + 1
  while (i < end) {
    const c = source[i]
    if (c === BS) {
      i += 2
      continue
    }
    if (c === BT) return i + 1
    if (c === '$' && source[i + 1] === '{') {
      const state = { i: i + 2, end, depth: 0, prevChar: '', prevWord: '' }
      scanJsRegion(source, state, comments)
      i = state.i
      continue
    }
    i++
  }
  return end
}

function skipJsRegex(source, start, end) {
  let i = start + 1
  let inClass = false
  while (i < end) {
    const c = source[i]
    if (c === BS) {
      i += 2
      continue
    }
    if (c === NL) return start
    if (inClass) {
      if (c === ']') inClass = false
      i++
      continue
    }
    if (c === '[') {
      inClass = true
      i++
      continue
    }
    if (c === '/') {
      i++
      while (i < end && /[a-z]/i.test(source[i])) i++
      return i
    }
    i++
  }
  return start
}

function scanCss(source, start, end) {
  const comments = []
  let i = start
  while (i < end) {
    const c = source[i]
    if (c === '/' && source[i + 1] === '*') {
      const from = i
      i += 2
      while (i < end) {
        if (source[i] === '*' && source[i + 1] === '/') {
          i += 2
          break
        }
        i++
      }
      comments.push({ style: 'block', start: from, end: i })
      continue
    }
    if (c === '/' && source[i + 1] === '/') {
      const from = i
      while (i < end && source[i] !== NL) i++
      comments.push({ style: 'line', start: from, end: i })
      continue
    }
    if (c === '"' || c === "'") {
      i = skipJsQuoted(source, i)
      continue
    }
    i++
  }
  return comments
}

function scanVue(source) {
  const comments = []
  let i = 0
  while (i < source.length) {
    const html = source.indexOf('<!--', i)
    const script = source.indexOf('<script', i)
    const style = source.indexOf('<style', i)
    const candidates = [
      { kind: 'html', at: html },
      { kind: 'script', at: script },
      { kind: 'style', at: style },
    ].filter((c) => c.at !== -1)
    if (candidates.length === 0) break
    candidates.sort((a, b) => a.at - b.at)
    const next = candidates[0]
    if (next.kind === 'html') {
      const close = source.indexOf('-->', next.at + 4)
      const end = close === -1 ? source.length : close + 3
      comments.push({ style: 'html', start: next.at, end })
      i = end
      continue
    }
    const openEnd = source.indexOf('>', next.at)
    if (openEnd === -1) break
    const closeTag = next.kind === 'script' ? '</script>' : '</style>'
    const contentStart = openEnd + 1
    const close = source.indexOf(closeTag, contentStart)
    const contentEnd = close === -1 ? source.length : close
    if (next.kind === 'script') {
      const found = scanJs(source, contentStart, contentEnd)
      for (const c of found) comments.push(c)
    } else {
      const found = scanCss(source, contentStart, contentEnd)
      for (const c of found) comments.push(c)
    }
    i = close === -1 ? source.length : close + closeTag.length
  }
  return comments
}

function scanShell(source) {
  const comments = []
  let i = 0
  const pendingHeredocs = []
  while (i < source.length) {
    const c = source[i]
    if (c === NL) {
      i++
      for (const heredoc of pendingHeredocs) {
        i = consumeHeredoc(source, i, heredoc)
      }
      pendingHeredocs.length = 0
      continue
    }
    if (c === '#' && numberSignStartsComment(source, i)) {
      if (i === 0 && source[i + 1] === '!') {
        while (i < source.length && source[i] !== NL) i++
        continue
      }
      const start = i
      while (i < source.length && source[i] !== NL) i++
      comments.push({ style: 'line', start, end: i })
      continue
    }
    if (c === BS) {
      i += 2
      continue
    }
    if (c === "'") {
      i = skipShellSingle(source, i)
      continue
    }
    if (c === '"') {
      i = skipShellDouble(source, i, comments)
      continue
    }
    if (c === BT) {
      i = skipShellBacktick(source, i, comments)
      continue
    }
    if (
      c === '<' &&
      source[i + 1] === '<' &&
      source[i + 2] !== '<' &&
      source[i + 2] !== '='
    ) {
      const heredoc = readHeredocStart(source, i)
      if (heredoc) {
        pendingHeredocs.push(heredoc)
        i = heredoc.after
        continue
      }
    }
    if (c === '$' && source[i + 1] === '(') {
      i = skipShellParen(source, i + 2, comments, true)
      continue
    }
    if (c === '$' && source[i + 1] === '{') {
      i = skipShellBraced(source, i + 2)
      continue
    }
    if (c === '(') {
      i = skipShellParen(source, i + 1, comments, false)
      continue
    }
    i++
  }
  return comments
}

function numberSignStartsComment(source, i) {
  if (i === 0) return true
  const prev = source[i - 1]
  return isSpace(prev) || ';|&({'.includes(prev)
}

function skipShellSingle(source, start) {
  let i = start + 1
  while (i < source.length && source[i] !== "'") i++
  return i < source.length ? i + 1 : source.length
}

function skipShellDouble(source, start, comments) {
  let i = start + 1
  while (i < source.length) {
    const c = source[i]
    if (c === BS) {
      i += 2
      continue
    }
    if (c === '"') return i + 1
    if (c === '$' && source[i + 1] === '(') {
      i = skipShellParen(source, i + 2, comments, true)
      continue
    }
    if (c === '$' && source[i + 1] === '{') {
      i = skipShellBraced(source, i + 2)
      continue
    }
    if (c === BT) {
      i = skipShellBacktick(source, i, comments)
      continue
    }
    i++
  }
  return source.length
}

function skipShellBacktick(source, start, comments) {
  let i = start + 1
  while (i < source.length) {
    const c = source[i]
    if (c === BS) {
      i += 2
      continue
    }
    if (c === BT) return i + 1
    if (c === '$' && source[i + 1] === '(') {
      i = skipShellParen(source, i + 2, comments, true)
      continue
    }
    i++
  }
  return source.length
}

function skipShellParen(source, start, comments, collectComments) {
  let depth = 1
  let i = start
  while (i < source.length) {
    const c = source[i]
    if (c === BS) {
      i += 2
      continue
    }
    if (c === "'") {
      i = skipShellSingle(source, i)
      continue
    }
    if (c === '"') {
      i = skipShellDouble(source, i, collectComments ? comments : [])
      continue
    }
    if (c === BT) {
      i = skipShellBacktick(source, i, collectComments ? comments : [])
      continue
    }
    if (c === '$' && source[i + 1] === '{') {
      i = skipShellBraced(source, i + 2)
      continue
    }
    if (c === '$' && source[i + 1] === '(') {
      i = skipShellParen(
        source,
        i + 2,
        collectComments ? comments : [],
        collectComments,
      )
      continue
    }
    if (c === '#' && numberSignStartsComment(source, i) && depth > 0) {
      const from = i
      while (i < source.length && source[i] !== NL) i++
      if (collectComments) comments.push({ style: 'line', start: from, end: i })
      continue
    }
    if (c === '(') {
      depth++
      i++
      continue
    }
    if (c === ')') {
      depth--
      i++
      if (depth === 0) return i
      continue
    }
    i++
  }
  return source.length
}

function skipShellBraced(source, start) {
  let depth = 1
  let i = start
  while (i < source.length) {
    const c = source[i]
    if (c === BS) {
      i += 2
      continue
    }
    if (c === '{') {
      depth++
      i++
      continue
    }
    if (c === '}') {
      depth--
      i++
      if (depth === 0) return i
      continue
    }
    i++
  }
  return source.length
}

function readHeredocStart(source, start) {
  let i = start + 2
  let stripTabs = false
  if (source[i] === '-') {
    stripTabs = true
    i++
  }
  while (source[i] === SPACE || source[i] === TAB) i++
  let delimiter = ''
  if (source[i] === "'" || source[i] === '"') {
    const quote = source[i]
    i++
    while (i < source.length && source[i] !== quote) {
      delimiter += source[i]
      i++
    }
    if (source[i] === quote) i++
  } else {
    while (
      i < source.length &&
      !isSpace(source[i]) &&
      ';|&()<>'.indexOf(source[i]) === -1
    ) {
      delimiter += source[i]
      i++
    }
  }
  if (!delimiter) return null
  return { delimiter, stripTabs, after: i }
}

function consumeHeredoc(source, start, heredoc) {
  let i = start
  while (i < source.length) {
    let lineEnd = source.indexOf(NL, i)
    if (lineEnd === -1) lineEnd = source.length
    let line = source.slice(i, lineEnd)
    if (heredoc.stripTabs) {
      while (line[0] === TAB) line = line.slice(1)
    }
    i = lineEnd < source.length ? lineEnd + 1 : source.length
    if (line === heredoc.delimiter) return i
  }
  return source.length
}

export function scanSource(file, source) {
  const ext = extname(file)
  if (ext === '.rs') return scanRust(source)
  if (ext === '.vue') return scanVue(source)
  if (ext === '.mjs' || ext === '.js' || ext === '.ts')
    return scanJs(source, 0, source.length)
  if (ext === '.sh') return scanShell(source)
  if (ext === '.scss') return scanCss(source, 0, source.length)
  return []
}

export function analyzeSource(file, source) {
  const comments = scanSource(file, source)
  const starts = lineStartsOf(source)
  const coverage = new Uint8Array(source.length)
  for (const comment of comments) {
    for (let k = comment.start; k < comment.end; k++) coverage[k] = 1
  }
  let totalLines = 0
  for (let k = 0; k < source.length; k++) {
    if (source[k] === NL) totalLines++
  }
  if (source.length > 0 && source[source.length - 1] !== NL) totalLines++
  let commentLines = 0
  let pureCommentLines = 0
  for (let index = 0; index < starts.length; index++) {
    const lineStart = starts[index]
    let lineEnd = source.indexOf(NL, lineStart)
    if (lineEnd === -1) lineEnd = source.length
    let hasComment = false
    let hasCode = false
    for (let k = lineStart; k < lineEnd; k++) {
      if (coverage[k]) hasComment = true
      else if (!isSpace(source[k])) hasCode = true
    }
    if (hasComment) commentLines++
    if (hasComment && !hasCode) pureCommentLines++
  }
  return {
    file,
    totalLines,
    blocks: comments.length,
    commentLines,
    pureCommentLines,
    comments: comments.map((comment) => ({
      style: comment.style,
      start: comment.start,
      end: comment.end,
      startLine: lineOf(starts, comment.start),
      endLine: lineOf(starts, Math.max(comment.start, comment.end - 1)),
      text: source.slice(comment.start, comment.end),
    })),
  }
}

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'))
  } catch {
    return null
  }
}

export function loadBaseline(path) {
  const data = readJson(path)
  if (!data || typeof data !== 'object') return {}
  const baseline = {}
  for (const key of Object.keys(data)) {
    if (typeof data[key] === 'number') baseline[key] = data[key]
  }
  return baseline
}

export function compareBaseline(current, baseline) {
  const violations = []
  for (const file of Object.keys(current).sort()) {
    const allowed = baseline[file] ?? 0
    const actual = current[file].commentLines
    if (actual > allowed) violations.push({ file, actual, allowed })
  }
  return violations
}

function formatReport(rows) {
  const lines = []
  const withComments = rows.filter((row) => row.commentLines > 0)
  withComments.sort((a, b) => b.commentLines - a.commentLines)
  const width = Math.max(...withComments.map((row) => row.file.length), 4)
  for (const row of withComments) {
    lines.push(
      row.file.padEnd(width) +
        '  ' +
        String(row.commentLines).padStart(6) +
        ' 行注释 / ' +
        String(row.pureCommentLines).padStart(6) +
        ' 行纯注释 / ' +
        String(row.blocks).padStart(5) +
        ' 块 / 共 ' +
        row.totalLines +
        ' 行',
    )
  }
  const totalLines = rows.reduce((sum, row) => sum + row.totalLines, 0)
  const commentLines = rows.reduce((sum, row) => sum + row.commentLines, 0)
  const pureCommentLines = rows.reduce(
    (sum, row) => sum + row.pureCommentLines,
    0,
  )
  const blocks = rows.reduce((sum, row) => sum + row.blocks, 0)
  lines.push('')
  lines.push(
    '合计：' +
      rows.length +
      ' 个文件，' +
      totalLines +
      ' 行代码，' +
      commentLines +
      ' 行含注释，其中 ' +
      pureCommentLines +
      ' 行是纯注释行，共 ' +
      blocks +
      ' 块注释。',
  )
  return lines.join(NL)
}

function parseArgs(argv) {
  const options = {
    mode: 'check',
    json: false,
    details: false,
    baseline: DEFAULT_BASELINE,
    allowIncrease: false,
    root: null,
  }
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]
    if (arg === '--check') options.mode = 'check'
    else if (arg === '--report') options.mode = 'report'
    else if (arg === '--update') options.mode = 'update'
    else if (arg === '--init') options.mode = 'init'
    else if (arg === '--json') options.json = true
    else if (arg === '--details') options.details = true
    else if (arg === '--allow-increase') options.allowIncrease = true
    else if (arg === '--baseline') options.baseline = argv[++i]
    else if (arg === '--root') options.root = argv[++i]
    else if (arg === '--help' || arg === '-h') options.mode = 'help'
    else {
      console.error('未知参数：' + arg)
      process.exit(2)
    }
  }
  return options
}

function main() {
  const options = parseArgs(process.argv.slice(2))
  if (options.mode === 'help') {
    console.log(
      [
        '用法：node scripts/check-comments.mjs [--check|--report|--update|--init] [--json] [--details]',
        '',
        '  --check    对照基线检查（默认）：注释行数不得高于基线。',
        '  --report   打印每个文件的注释统计。',
        '  --update   把基线下调到当前水平（只允许下降）。',
        '  --init     用当前水平创建基线（首次引入时用）。',
        '  --json     --report/--details 时输出 JSON。',
        '  --details  输出每块注释的位置与原文。',
        '  --baseline 基线文件路径，默认 scripts/comment-baseline.json。',
        '  --root     仓库根目录，默认用 git rev-parse 得到。',
      ].join(NL),
    )
    return
  }
  const root = options.root ?? repoRoot()
  const files = collectFiles(root)
  const rows = []
  for (const file of files) {
    const source = readFileSync(join(root, file), 'utf8')
    rows.push(analyzeSource(file, source))
  }
  if (options.mode === 'report') {
    if (options.json) {
      console.log(JSON.stringify(rows, null, 2))
    } else {
      console.log(formatReport(rows))
    }
    return
  }
  if (options.details) {
    const details = []
    for (const row of rows) {
      for (const comment of row.comments) {
        details.push({ ...comment, file: row.file })
      }
    }
    console.log(JSON.stringify(details, null, 2))
    return
  }
  const current = {}
  for (const row of rows) current[row.file] = row
  const baselinePath = join(root, options.baseline)
  if (options.mode === 'init') {
    const baseline = {}
    for (const row of rows)
      if (row.commentLines > 0) baseline[row.file] = row.commentLines
    writeFileSync(
      baselinePath,
      JSON.stringify(sortKeys(baseline), null, 2) + NL,
    )
    const total = Object.values(baseline).reduce((sum, count) => sum + count, 0)
    console.log(
      '注释基线已创建：' +
        Object.keys(baseline).length +
        ' 个文件，' +
        total +
        ' 行注释。',
    )
    return
  }
  const baseline = loadBaseline(baselinePath)
  if (options.mode === 'update') {
    const violations = compareBaseline(current, baseline)
    if (violations.length > 0 && !options.allowIncrease) {
      console.error('以下文件注释增加了，基线不能上调：')
      for (const violation of violations) {
        console.error(
          '  ' +
            violation.file +
            '：' +
            violation.actual +
            ' > ' +
            violation.allowed,
        )
      }
      console.error('如果确实要上调基线，显式加 --allow-increase。')
      process.exit(1)
    }
    const next = {}
    for (const row of rows)
      if (row.commentLines > 0) next[row.file] = row.commentLines
    writeFileSync(baselinePath, JSON.stringify(sortKeys(next), null, 2) + NL)
    const before = Object.values(baseline).reduce(
      (sum, count) => sum + count,
      0,
    )
    const after = Object.values(next).reduce((sum, count) => sum + count, 0)
    console.log(
      '注释基线已更新：' +
        Object.keys(baseline).length +
        ' 个文件 / ' +
        before +
        ' 行 → ' +
        Object.keys(next).length +
        ' 个文件 / ' +
        after +
        ' 行。',
    )
    return
  }
  const violations = compareBaseline(current, baseline)
  if (violations.length > 0) {
    console.error('注释基线被突破（禁止新增注释）：')
    for (const violation of violations) {
      console.error(
        '  ' +
          violation.file +
          '：当前 ' +
          violation.actual +
          ' 行，基线 ' +
          violation.allowed +
          ' 行。',
      )
    }
    console.error(
      '删掉新增的注释；确实需要给某个文件留注释时，先把它记进基线并说明理由。',
    )
    process.exit(1)
  }
  const total = rows.reduce((sum, row) => sum + row.commentLines, 0)
  console.log(
    '注释基线：通过（' +
      Object.keys(baseline).length +
      ' 个文件，' +
      total +
      ' 行注释）。',
  )
}

function sortKeys(object) {
  const sorted = {}
  for (const key of Object.keys(object).sort()) sorted[key] = object[key]
  return sorted
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main()
}

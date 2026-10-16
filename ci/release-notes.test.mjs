// Release-note classifier/linking tests (macOS-only artifacts).
// Run: node --test ci/release-notes.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { classify, render } from './release-notes-render.mjs'

const TAG = 'v0.1.0-dev.42'
const REPO = 'acme/dsh-launcher'

const COMMITS = [
  'feat: add theme switcher',
  'fix: hide window on close',
  'ci: build macos dmg',
]

const ASSETS = [
  'dsh-launcher_0.1.0_aarch64.dmg',
  'dsh-launcher_0.1.0_arm64.dmg',
]

test('classifier maps the macOS aarch64 dmg artifact', () => {
  for (const name of ASSETS) {
    const c = classify(name)
    assert.ok(c, `expected ${name} to classify`)
    assert.equal(c.arch, 'arm64')
    assert.equal(c.kind, 'dmg')
  }
})

test('classifier rejects unknown or non-macOS artifacts', () => {
  for (const bad of [
    'README.md',
    'dsh-launcher_0.1.0_x64.dmg', // Intel builds are no longer produced
    'dsh-launcher_0.1.0_x64-setup.exe',
    'dsh-launcher_0.1.0_amd64.deb',
    'dsh-launcher-0.1.0-1.x86_64.rpm',
    'totally-unrelated.zip',
  ]) {
    assert.equal(classify(bad), null, `${bad} must not classify`)
  }
})

test('every artifact row is a clickable link to the release asset URL', () => {
  const body = render(TAG, ASSETS, COMMITS, { repo: REPO })
  for (const name of ASSETS) {
    const url = `https://github.com/${REPO}/releases/download/${TAG}/${name}`
    const row = new RegExp(
      `\\| [^|]+ \\| [^|]+ \\| \\[${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\]\\(${url.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\)`,
    )
    assert.match(body, row, `expected a clickable row for ${name}`)
  }
})

test('rendered body has English-only table and a commit list', () => {
  const body = render(TAG, ASSETS, COMMITS, { repo: REPO })
  assert.ok(body.includes('## Downloads'))
  assert.ok(body.includes('| Platform | Architecture | File |'))
  assert.ok(body.includes("## What's Changed"))
  for (const c of COMMITS) {
    assert.ok(body.includes(`* ${c}`), `expected commit line for ${c}`)
  }
  // English-only: no Chinese section headers or labels.
  assert.ok(!body.includes('简体中文'))
  assert.ok(!body.includes('安装包'))
})

test("empty commit list renders an empty What's Changed section", () => {
  const body = render(TAG, ASSETS, [], { repo: REPO })
  assert.ok(body.includes("## What's Changed"))
})

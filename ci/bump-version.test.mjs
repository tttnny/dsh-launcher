// Bump-version tests: the script must only bump stable tags and must leave
// prerelease/dev tags untouched (idempotent, no-op).
// Run: node --test ci/bump-version.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import {
  mkdirSync,
  mkdtempSync,
  writeFileSync,
  readFileSync,
  rmSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

function runInTempDir(tag, manifestVersion = tag.replace(/^v/, '')) {
  const dir = mkdtempSync(join(tmpdir(), 'dsh-bump-'))
  const files = {
    'package.json':
      JSON.stringify(
        { name: 'dsh-launcher', version: manifestVersion },
        null,
        2,
      ) + '\n',
    'src-tauri/tauri.conf.json':
      JSON.stringify({ version: manifestVersion }, null, 2) + '\n',
    'src-tauri/Cargo.toml': `name = "dsh-launcher"\nversion = "${manifestVersion}"\n`,
    'src-tauri/Cargo.lock': `[[package]]\nname = "dsh-launcher"\nversion = "${manifestVersion}"\n`,
  }
  for (const [p, content] of Object.entries(files)) {
    const full = join(dir, p)
    const parent = join(dir, p.split('/')[0])
    if (p.includes('/')) mkdirSync(parent, { recursive: true })
    writeFileSync(full, content)
  }
  const res = spawnSync(
    process.execPath,
    [join(process.cwd(), 'ci', 'bump-version.mjs'), tag],
    {
      cwd: dir,
      encoding: 'utf8',
    },
  )
  // Read every file BEFORE deleting the temp dir; the returned map is what
  // the assertions inspect.
  const contents = {}
  for (const p of Object.keys(files)) {
    contents[p] = readFileSync(join(dir, p), 'utf8')
  }
  rmSync(dir, { recursive: true, force: true })
  return { res, contents }
}

test('stable tag bumps patch version across all four manifests', () => {
  const { res, contents } = runInTempDir('v0.2.4')
  assert.equal(res.status, 0, res.stderr)
  assert.match(res.stdout, /bumped 0\.2\.4 -> 0\.2\.5/)
  assert.match(contents['package.json'], /"version": "0\.2\.5"/)
  assert.match(contents['src-tauri/tauri.conf.json'], /"version": "0\.2\.5"/)
  assert.match(contents['src-tauri/Cargo.toml'], /^version = "0\.2\.5"/m)
  assert.match(contents['src-tauri/Cargo.lock'], /^version = "0\.2\.5"/m)
})

test('0.3.0 bumps to 0.3.1 and 1.0.0 to 1.0.1', () => {
  for (const [from, to] of [
    ['v0.3.0', '0.3.1'],
    ['v1.0.0', '1.0.1'],
  ]) {
    const { res, contents } = runInTempDir(from)
    assert.equal(res.status, 0, res.stderr)
    assert.match(contents['package.json'], new RegExp(`"version": "${to}"`))
  }
})

test('prerelease tags are a no-op', () => {
  for (const tag of ['v0.2.4-dev.12', 'v1.0.0-rc.1']) {
    const { res, contents } = runInTempDir(tag, '0.2.4')
    assert.equal(res.status, 0, res.stderr)
    assert.match(res.stdout, /not a stable release; no bump/)
    assert.match(contents['package.json'], /"version": "0\.2\.4"/)
    assert.match(contents['src-tauri/Cargo.toml'], /^version = "0\.2\.4"/m)
  }
})

test('non-v prefix is rejected', () => {
  const { res } = runInTempDir('0.2.4')
  assert.notEqual(res.status, 0)
  assert.match(res.stderr, /usage: node ci\/bump-version\.mjs/)
})

test('already-bumped manifests are left untouched (idempotent)', () => {
  const dir = mkdtempSync(join(tmpdir(), 'dsh-bump-'))
  const files = {
    'package.json':
      JSON.stringify({ name: 'dsh-launcher', version: '0.2.5' }, null, 2) +
      '\n',
    'src-tauri/tauri.conf.json':
      JSON.stringify({ version: '0.2.5' }, null, 2) + '\n',
    'src-tauri/Cargo.toml': 'name = "dsh-launcher"\nversion = "0.2.5"\n',
    'src-tauri/Cargo.lock':
      '[[package]]\nname = "dsh-launcher"\nversion = "0.2.5"\n',
  }
  for (const [p, content] of Object.entries(files)) {
    const full = join(dir, p)
    const parent = join(dir, p.split('/')[0])
    if (p.includes('/')) mkdirSync(parent, { recursive: true })
    writeFileSync(full, content)
  }
  const res = spawnSync(
    process.execPath,
    [join(process.cwd(), 'ci', 'bump-version.mjs'), 'v0.2.4'],
    {
      cwd: dir,
      encoding: 'utf8',
    },
  )
  assert.equal(res.status, 0, res.stderr)
  assert.match(
    res.stdout,
    /manifest version 0\.2\.5 != released 0\.2\.4; no bump/,
  )
  assert.match(
    readFileSync(join(dir, 'package.json'), 'utf8'),
    /"version": "0\.2\.5"/,
  )
  rmSync(dir, { recursive: true, force: true })
})

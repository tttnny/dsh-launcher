// The 「查看命令」 a user copies out of the delete-host dialog — and the same line
// the instance card copies — has to survive two shells on the way to the machine:
// the user's own shell parses it into one argument for ssh, and the login shell
// at the other end splits that argument again. Both shells are real here, and
// HOME is a throwaway directory, so `~` means something this test controls.
//
// What it guards: a remote path spelled `~/…` must still expand. Quoted, a tilde
// is a literal tilde, and the command then asks the far side for a directory
// named `~` — the whole point of the line is lost. A path holding a space or a
// single quote must survive too, which is why the two are tested together.
//
// Run: node --test ci/remote-command.test.mjs

import test from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const { directoryCommand } = await import('../src/remoteCommand.ts')

/** A throwaway HOME holding `<name>/<marker>`, to look at and nothing else. */
function homeWith(name, marker) {
  const home = mkdtempSync(join(tmpdir(), 'dsh-remote-command-'))
  const dir = name ? join(home, name) : home
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, marker), '')
  return home
}

/** The words the user's own shell hands ssh. One argument is the whole point. */
function wordsHandedToSsh(command) {
  const out = spawnSync('/bin/sh', ['-c', `printf '%s\n' ${command}`], { encoding: 'utf8' })
  assert.equal(out.status, 0, out.stderr)
  return out.stdout.split('\n').filter((line) => line.length > 0)
}

/** What that machine does with them: ssh joins its arguments, its login shell splits them again. */
function onTheOtherMachine(words, home) {
  return spawnSync('/bin/sh', ['-c', words.slice(2).join(' ')], {
    encoding: 'utf8',
    env: { ...process.env, HOME: home },
  })
}

test('a ~/ path expands on the far side instead of naming a directory called ~', () => {
  const home = homeWith('dsh data', 'found-here')

  const words = wordsHandedToSsh(directoryCommand('lab', '~/dsh data'))
  assert.equal(words.length, 3, `ssh 只该拿到一个参数：${JSON.stringify(words)}`)

  const out = onTheOtherMachine(words, home)
  assert.equal(out.status, 0, out.stderr)
  assert.match(out.stdout, /found-here/, out.stdout + out.stderr)
})

test('a bare ~ is that machine\'s home directory', () => {
  const home = homeWith('dsh data', 'found-here')

  const words = wordsHandedToSsh(directoryCommand('lab', '~'))

  const out = onTheOtherMachine(words, home)
  assert.equal(out.status, 0, out.stderr)
  assert.match(out.stdout, /dsh data/, out.stdout + out.stderr)
})

test('an absolute path holding a space and a single quote arrives as one word', () => {
  const home = homeWith("o'brien dir", 'found-here')
  const path = join(home, "o'brien dir")

  const words = wordsHandedToSsh(directoryCommand('lab', path))
  assert.equal(words.length, 3, `ssh 只该拿到一个参数：${JSON.stringify(words)}`)

  const out = onTheOtherMachine(words, home)
  assert.equal(out.status, 0, out.stderr)
  assert.match(out.stdout, /found-here/, out.stdout + out.stderr)
})

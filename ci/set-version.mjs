// Stamps a build version into every manifest that embeds it:
// package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml and the
// workspace package entry in src-tauri/Cargo.lock. CI release builds call
// this so dev artifacts carry their real v<ver>-dev.<run> version instead of
// the plain manifest version.
import { readFileSync, writeFileSync } from 'node:fs'

const version = process.argv[2]
if (
  !version ||
  !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$/.test(version)
) {
  console.error(
    `usage: node ci/set-version.mjs <semver> (got "${version ?? ''}")`,
  )
  process.exit(1)
}

// package.json + tauri.conf.json (plain JSON, 2-space indent like the repo).
for (const path of ['package.json', 'src-tauri/tauri.conf.json']) {
  const doc = JSON.parse(readFileSync(path, 'utf8'))
  doc.version = version
  writeFileSync(path, `${JSON.stringify(doc, null, 2)}\n`)
}

// Cargo.toml: the first `version = "…"` belongs to [package]. Presence is
// asserted on the pattern, NOT on the replacement changing the text: a
// release build stamps the version the manifest already carries (tag builds
// pass the manifest version verbatim), so an unchanged file is the normal
// no-op case, not a missing key.
const tomlPath = 'src-tauri/Cargo.toml'
const toml = readFileSync(tomlPath, 'utf8')
const tomlVersionRe = /^version = "[^"]+"/m
if (!tomlVersionRe.test(toml)) {
  console.error(`${tomlPath}: package version key not found`)
  process.exit(1)
}
writeFileSync(tomlPath, toml.replace(tomlVersionRe, `version = "${version}"`))

// Cargo.lock: update only this crate's [[package]] entry so `--locked`
// builds keep working after the stamp. Windows checkouts may use CRLF line
// endings, so the pattern tolerates \r\n and preserves it via $1.
const pkgName = toml.match(/^name = "([^"]+)"/m)?.[1]
if (!pkgName) {
  console.error(`${tomlPath}: package name not found`)
  process.exit(1)
}
const lockPath = 'src-tauri/Cargo.lock'
const lock = readFileSync(lockPath, 'utf8')
const blockRe = new RegExp(
  `(\\[\\[package\\]\\]\\r?\\nname = "${pkgName}"\\r?\\nversion = ")[^"]+"`,
)
if (!blockRe.test(lock)) {
  console.error(`${lockPath}: workspace package entry not found`)
  process.exit(1)
}
writeFileSync(lockPath, lock.replace(blockRe, `$1${version}"`))

console.log(`stamped version ${version}`)

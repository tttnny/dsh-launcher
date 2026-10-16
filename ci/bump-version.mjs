// Bumps the manifest version to the next patch release after a stable tag is
// shipped: 0.2.4 -> 0.2.5, 0.3.0 -> 0.3.1, 1.0.0 -> 1.0.1.
//
// The version stamped is the released tag WITHOUT the pre-release segment and
// with the patch incremented; prerelease/dev tags never trigger a bump.
// The script rewrites package.json / src-tauri/tauri.conf.json / Cargo.toml /
// Cargo.lock so the next `v<version>` tag is one patch ahead.
//
// Run: node ci/bump-version.mjs <released-tag>   (e.g. v0.2.4)
import { readFileSync, writeFileSync } from 'node:fs'

const tag = process.argv[2]
if (!tag || !tag.startsWith('v')) {
  console.error(`usage: node ci/bump-version.mjs <vX.Y.Z> (got "${tag ?? ''}")`)
  process.exit(1)
}
const version = tag.slice(1)
const semverRe = /^\d+\.\d+\.\d+$/
if (!semverRe.test(version)) {
  // v0.2.4-dev.12 / v1.0.0-rc.1 etc.: not a stable release, nothing to bump.
  console.log(`tag ${tag} is not a stable release; no bump`)
  process.exit(0)
}
const [major, minor, patch] = version.split('.').map(Number)
const next = `${major}.${minor}.${patch + 1}`

// Idempotence guard: only bump when the checked-out manifests still carry
// the released version. A re-run of the job (or a manual bump) must never
// downgrade or double-advance the version.
const current = JSON.parse(readFileSync('package.json', 'utf8')).version
if (current !== version) {
  console.log(`manifest version ${current} != released ${version}; no bump`)
  process.exit(0)
}

for (const path of ['package.json', 'src-tauri/tauri.conf.json']) {
  const doc = JSON.parse(readFileSync(path, 'utf8'))
  doc.version = next
  writeFileSync(path, `${JSON.stringify(doc, null, 2)}\n`)
}

const tomlPath = 'src-tauri/Cargo.toml'
const toml = readFileSync(tomlPath, 'utf8')
const tomlVersionRe = /^version = "[^"]+"/m
if (!tomlVersionRe.test(toml)) {
  console.error(`${tomlPath}: package version key not found`)
  process.exit(1)
}
writeFileSync(tomlPath, toml.replace(tomlVersionRe, `version = "${next}"`))

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
writeFileSync(lockPath, lock.replace(blockRe, `$1${next}"`))

console.log(`bumped ${version} -> ${next}`)

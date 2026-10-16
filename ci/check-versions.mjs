// Ensures the workspace, package, and native app manifests stay in sync.
// Errors (exit != 0) whenever any version drifts.
import { readFileSync } from 'node:fs'

const pkgVersion = JSON.parse(readFileSync('package.json', 'utf8')).version
const cargoVersion = readFileSync('src-tauri/Cargo.toml', 'utf8').match(
  /^version = "([^"]+)"/m,
)?.[1]
const tauriVersion = JSON.parse(
  readFileSync('src-tauri/tauri.conf.json', 'utf8'),
).version

const errors = []
if (cargoVersion !== pkgVersion)
  errors.push(
    `src-tauri/Cargo.toml version ${cargoVersion} != package.json ${pkgVersion}`,
  )
if (tauriVersion !== pkgVersion)
  errors.push(
    `src-tauri/tauri.conf.json version ${tauriVersion} != package.json ${pkgVersion}`,
  )

if (errors.length > 0) {
  console.error(errors.join('\n'))
  process.exit(1)
}
console.log(`versions in sync: ${pkgVersion}`)

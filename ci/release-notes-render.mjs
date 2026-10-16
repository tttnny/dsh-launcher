// Release-note renderer + artifact classifier (macOS-only builds).
// Shared by ci/update-release-notes.sh (CLI) and ci/release-notes.test.mjs (tests).

/**
 * Maps a release asset file name to a { platform, arch, kind, name } record,
 * or null when the name does not match the macOS DMG artifact. Only Apple
 * Silicon DMGs are produced (dsh-launcher_<version>_aarch64.dmg).
 */
export function classify(name) {
  const m = /^dsh-launcher_\d[\w.-]*_(aarch64|arm64)\.dmg$/.exec(name)
  if (!m) return null
  return { name, arch: 'arm64', kind: 'dmg' }
}

const PLATFORM_OF_KIND = { dmg: 'macOS' }
const KIND_LABEL_EN = { dmg: 'DMG' }
const KIND_ORDER = { dmg: 0 }

/**
 * Renders the English-only "Downloads" table plus the "What's Changed"
 * commit list. Every artifact row links directly to its release-asset
 * download URL.
 */
export function render(tag, assets, commits, { repo = 'REPO' } = {}) {
  const sorted = [...assets]
    .filter((a) => classify(a))
    .sort((a, b) => {
      const c1 = classify(a)
      const c2 = classify(b)
      if (c1.arch !== c2.arch) return c1.arch.localeCompare(c2.arch)
      return KIND_ORDER[c1.kind] - KIND_ORDER[c2.kind]
    })

  const link = (name) =>
    `https://github.com/${repo}/releases/download/${tag}/${name}`
  const rows = sorted.map((name) => {
    const c = classify(name)
    return `| ${PLATFORM_OF_KIND[c.kind]} | ${c.arch} | [${name}](${link(name)}) (${KIND_LABEL_EN[c.kind]}) |`
  })

  const commitLines = (commits ?? []).map((c) => `* ${c}`).join('\n')

  return `## Downloads

| Platform | Architecture | File |
| --- | --- | --- |
${rows.join('\n')}

---

## What's Changed

${commitLines}
`
}

// CLI mode: node ci/release-notes-render.mjs <tag> <assets-json> <commits-file>
import { fileURLToPath, pathToFileURL } from 'node:url'
import { readFileSync } from 'node:fs'
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const [, , tag, assetsJson, commitsFile] = process.argv
  const assets = JSON.parse(assetsJson)
  const commits = commitsFile
    ? readFileSync(commitsFile, 'utf8')
        .split('\n')
        .map((s) => s.trim())
        .filter(Boolean)
    : []
  const repo = process.env.GITHUB_REPOSITORY ?? 'REPO'
  const unknown = assets.filter((a) => !classify(a))
  if (unknown.length > 0) {
    console.error(
      `release-note classifier rejected assets: ${unknown.join(', ')}`,
    )
    process.exit(1)
  }
  process.stdout.write(render(tag, assets, commits, { repo }))
}

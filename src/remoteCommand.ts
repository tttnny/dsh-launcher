function word(value: string): string {
  return `'${value.replace(/'/g, "'\\''")}'`
}

function pathWord(path: string): string {
  if (path === '~') return '~'
  if (path.startsWith('~/')) return `~/${word(path.slice(2))}`
  return word(path)
}

export function directoryCommand(target: string, path: string): string {
  return `ssh ${target} ${word(`/bin/bash -ilc "ls -la ${pathWord(path)}"`)}`
}

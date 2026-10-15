// The copyable `ssh …` line for a directory on another machine.
//
// The backend builds this same line for an instance's card (`remote::ssh_command`
// with `commands::directory_recipe`), and the two must agree: the user copies it
// from either place. Two shells read it — this Mac's, which parses the line that
// was pasted, and the login shell at the other end, which splits the single word
// it got from ssh again. `word` below is the backend's `remote::quote`.
//
// The one thing that is not plain quoting is `~`. Inside quotes a tilde is a
// literal tilde: a path the user can still spell `~/…` — the host form takes one,
// and the delete dialog's fallback is exactly that — would ask for a directory
// named `~`. So a leading `~/` stays outside the quotes and only the rest of the
// path is quoted (`~/'我的 目录'`): the tilde expands, and a path holding a space
// or a single quote still arrives as one word. Only `~` and `~/…` expand;
// `~用户名/…` is a spelling the launcher does not support anywhere (the backend's
// `home_path_script` refuses it), so it stays a literal word here too.
//
// A path holding a double quote is the one thing this cannot carry: the inner
// pair of quotes around the script would end early. The backend's recipes have
// the same limit, which is why it writes them without one.

/** POSIX single-quoting — the backend's `remote::quote`. */
function word(value: string): string {
  return `'${value.replace(/'/g, "'\\''")}'`
}

/** One path as one word for the shell at the other end. */
function pathWord(path: string): string {
  if (path === '~') return '~'
  if (path.startsWith('~/')) return `~/${word(path.slice(2))}`
  return word(path)
}

/**
 * What the user pastes into their own shell to look at that directory: the ssh
 * target, and `ls -la` at the path run through that machine's login shell.
 */
export function directoryCommand(target: string, path: string): string {
  return `ssh ${target} ${word(`/bin/bash -ilc "ls -la ${pathWord(path)}"`)}`
}

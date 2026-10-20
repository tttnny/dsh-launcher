//! The `dsh` command on this Mac's PATH.
//!
//! A version the launcher installs lives under the launcher's data directory,
//! which no PATH folder reaches. Tools that look a CLI up by name — magpie, a
//! shell — only see one when `dsh` sits in a PATH folder. Registering a version
//! writes one link at `~/.local/bin/dsh`: that is the folder magpie adds to its
//! own PATH and the one a user's shell normally has. One link, one version;
//! registering another replaces it.
//!
//! A real `dsh` already there (npm's global bin, an installer's own link) is
//! never overwritten or removed: the launcher says so and leaves it alone.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch;

/// The line a wrapper script carries, with the version directory after it. A
/// script is only written for a CLI that is not executable itself. The remote
/// script (`crate::remote`'s `PATH_LINK`) writes and recognises the same line.
pub const MARKER: &str = "# dsh-launcher path link: ";

/// What `~/.local/bin/dsh` is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// Nothing there: no version is registered.
    Absent,
    /// A symbolic link, to the path it names (a relative target is resolved
    /// against the link's own folder).
    Symlink(PathBuf),
    /// A wrapper script the launcher wrote, naming the version directory it
    /// runs.
    Script(PathBuf),
    /// Something else: a real file, a folder, or a link of the user's. It is
    /// never overwritten and never removed.
    Foreign,
}

/// This Mac's home directory, where `~/.local/bin` lives.
pub fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "读不到本机的 HOME，无法定位 ~/.local/bin".to_string())
}

pub fn link_path(home: &Path) -> PathBuf {
    home.join(".local").join("bin").join("dsh")
}

pub fn inspect(home: &Path) -> LinkState {
    let link = link_path(home);
    let Ok(meta) = fs::symlink_metadata(&link) else {
        return LinkState::Absent;
    };
    if meta.file_type().is_symlink() {
        let Ok(target) = fs::read_link(&link) else {
            return LinkState::Foreign;
        };
        let target = if target.is_absolute() {
            target
        } else {
            link.parent().unwrap_or(Path::new("/")).join(target)
        };
        return LinkState::Symlink(target);
    }
    if !meta.is_file() {
        return LinkState::Foreign;
    }
    let Ok(text) = fs::read_to_string(&link) else {
        return LinkState::Foreign;
    };
    text.lines()
        .take(3)
        .find_map(|line| line.strip_prefix(MARKER))
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(|dir| LinkState::Script(PathBuf::from(dir)))
        .unwrap_or(LinkState::Foreign)
}

/// The version directory the link runs, when it is one of `versions`.
pub fn linked_to(state: &LinkState, versions: &[&Path]) -> Option<PathBuf> {
    match state {
        LinkState::Symlink(target) => versions
            .iter()
            .copied()
            .find(|dir| target.starts_with(dir))
            .map(Path::to_path_buf),
        LinkState::Script(dir) => versions
            .iter()
            .copied()
            .find(|version| crate::config::paths_equal(version, dir))
            .map(Path::to_path_buf),
        LinkState::Absent | LinkState::Foreign => None,
    }
}

/// Registers one version as the `dsh` command on PATH.
///
/// `versions` is every version the launcher knows on this Mac: a link into one
/// of them is the launcher's to replace, and anything else is the user's.
pub fn register(home: &Path, version_dir: &Path, versions: &[&Path]) -> Result<(), String> {
    let state = inspect(home);
    ensure_replaceable(&state, versions)?;

    let entry = launch::version_bin_dir(version_dir).join("dsh");
    let bin = launch::version_bin(version_dir);
    let target = if entry.is_file() { entry } else { bin };
    if !target.is_file() {
        return Err(format!(
            "这个版本没有 dsh 启动文件（{}），先把它装好再登记",
            target.display()
        ));
    }

    let link = link_path(home);
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("建立 {} 失败：{e}", parent.display()))?;
    }
    if fs::symlink_metadata(&link).is_ok() {
        fs::remove_file(&link).map_err(|e| format!("替换 {} 失败：{e}", link.display()))?;
    }

    if executable(&target) {
        symlink(&target, &link)
    } else {
        write_script(&link, version_dir, &target)
    }
}

/// Removes the launcher's registration, if there is one. A `dsh` the user
/// installed is not touched.
pub fn unregister(home: &Path, versions: &[&Path]) -> Result<(), String> {
    let state = inspect(home);
    match state {
        LinkState::Absent => Ok(()),
        _ if linked_to(&state, versions).is_some() => remove_link(home),
        LinkState::Symlink(target) => Err(format!(
            "~/.local/bin/dsh 指向 {}，不是启动器登记的版本，没有改动它",
            target.display()
        )),
        LinkState::Script(_) | LinkState::Foreign => {
            Err("~/.local/bin/dsh 不是启动器登记的，没有改动它".to_string())
        }
    }
}

/// Removes the registration only while it runs `version_dir`: deleting a
/// version withdraws its own link, never another version's.
pub fn unlink_version(home: &Path, version_dir: &Path) -> Result<(), String> {
    let state = inspect(home);
    let ours = match &state {
        LinkState::Symlink(target) => target.starts_with(version_dir),
        LinkState::Script(dir) => crate::config::paths_equal(dir, version_dir),
        LinkState::Absent | LinkState::Foreign => false,
    };
    if ours {
        remove_link(home)
    } else {
        Ok(())
    }
}

fn ensure_replaceable(state: &LinkState, versions: &[&Path]) -> Result<(), String> {
    match state {
        LinkState::Absent => Ok(()),
        _ if linked_to(state, versions).is_some() => Ok(()),
        LinkState::Symlink(target) => Err(format!(
            "~/.local/bin/dsh 已经指向 {}，不是启动器安装的版本；先把它移走再登记",
            target.display()
        )),
        LinkState::Script(_) | LinkState::Foreign => {
            Err("~/.local/bin/dsh 已经存在，而且不是启动器登记的；先把它移走再登记".to_string())
        }
    }
}

fn remove_link(home: &Path) -> Result<(), String> {
    let link = link_path(home);
    match fs::remove_file(&link) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除 {} 失败：{e}", link.display())),
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|e| format!("在 {} 建立 dsh 命令失败：{e}", link.display()))
}

#[cfg(not(unix))]
fn symlink(_target: &Path, link: &Path) -> Result<(), String> {
    Err(format!(
        "在 {} 建立 dsh 命令失败：这个系统不支持符号链接",
        link.display()
    ))
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}

fn write_script(link: &Path, version_dir: &Path, bin: &Path) -> Result<(), String> {
    let body = format!(
        "#!/bin/sh\n{MARKER}{}\nexec node {} \"$@\"\n",
        version_dir.display(),
        shell_quote(&bin.to_string_lossy())
    );
    fs::write(link, body).map_err(|e| format!("在 {} 建立 dsh 命令失败：{e}", link.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(link, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("设置 {} 的可执行权限失败：{e}", link.display()))?;
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsh-pathlink-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn version_dir(root: &Path, name: &str, executable: bool) -> PathBuf {
        let dir = root.join(name);
        let bin = launch::version_bin(&dir);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/usr/bin/env node\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        dir
    }

    #[test]
    fn registering_a_version_creates_the_link_and_names_it() {
        let home = scratch("home");
        let root = scratch("versions");
        let version = version_dir(&root, "0.2.1-rc.1", true);

        register(&home, &version, &[&version]).unwrap();

        let state = inspect(&home);
        assert!(matches!(state, LinkState::Symlink(_)), "{state:?}");
        assert_eq!(linked_to(&state, &[&version]), Some(version.clone()));
        assert_eq!(
            std::fs::read_link(link_path(&home)).unwrap(),
            launch::version_bin(&version)
        );

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn registering_another_version_replaces_the_launchers_link() {
        let home = scratch("home");
        let root = scratch("versions");
        let first = version_dir(&root, "0.2.1-rc.1", true);
        let second = version_dir(&root, "0.2.2-rc.1", true);

        register(&home, &first, &[&first]).unwrap();
        register(&home, &second, &[&first, &second]).unwrap();

        let state = inspect(&home);
        assert_eq!(linked_to(&state, &[&first, &second]), Some(second.clone()));
        assert_eq!(linked_to(&state, &[&first]), None);

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_real_dsh_of_the_users_is_never_overwritten() {
        let home = scratch("home");
        let root = scratch("versions");
        let version = version_dir(&root, "0.2.1-rc.1", true);
        let link = link_path(&home);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::fs::write(&link, "#!/bin/sh\necho system dsh\n").unwrap();

        let err = register(&home, &version, &[&version]).unwrap_err();

        assert!(err.contains("不是启动器"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&link).unwrap(),
            "#!/bin/sh\necho system dsh\n"
        );
        assert_eq!(inspect(&home), LinkState::Foreign);

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_to_somewhere_else_is_not_replaced_or_removed() {
        let home = scratch("home");
        let root = scratch("versions");
        let version = version_dir(&root, "0.2.1-rc.1", true);
        let link = link_path(&home);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/usr/bin/true", &link).unwrap();

        assert!(register(&home, &version, &[&version]).is_err());
        assert!(unregister(&home, &[&version]).is_err());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            PathBuf::from("/usr/bin/true")
        );

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn deleting_a_version_removes_only_its_own_link() {
        let home = scratch("home");
        let root = scratch("versions");
        let first = version_dir(&root, "0.2.1-rc.1", true);
        let second = version_dir(&root, "0.2.2-rc.1", true);

        register(&home, &first, &[&first]).unwrap();
        unlink_version(&home, &second).unwrap();
        assert!(matches!(inspect(&home), LinkState::Symlink(_)));

        unlink_version(&home, &first).unwrap();
        assert_eq!(inspect(&home), LinkState::Absent);

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_cli_that_is_not_executable_gets_a_wrapper_script() {
        let home = scratch("home");
        let root = scratch("versions");
        let version = version_dir(&root, "0.2.1-rc.1", false);

        register(&home, &version, &[&version]).unwrap();

        let state = inspect(&home);
        assert_eq!(state, LinkState::Script(version.clone()));
        assert_eq!(linked_to(&state, &[&version]), Some(version.clone()));
        let body = std::fs::read_to_string(link_path(&home)).unwrap();
        assert!(body.starts_with("#!/bin/sh\n"), "{body}");
        assert!(body.contains("exec node "), "{body}");

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unregistering_with_nothing_there_is_not_an_error() {
        let home = scratch("home");
        let root = scratch("versions");
        let version = version_dir(&root, "0.2.1-rc.1", true);

        unregister(&home, &[&version]).unwrap();
        assert_eq!(inspect(&home), LinkState::Absent);

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}

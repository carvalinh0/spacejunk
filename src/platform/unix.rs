//! Linux and macOS: `read_dir` plus an lstat per entry.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use super::{Entry, Root};

/// `st_blocks` is always in 512-byte units, regardless of the fs block size.
fn disk_usage(meta: &fs::Metadata) -> u64 {
    meta.blocks() * 512
}

pub fn root(path: &Path) -> io::Result<Root> {
    let meta = fs::symlink_metadata(path)?;
    Ok(Root {
        is_dir: meta.is_dir(),
        size: disk_usage(&meta),
        dev: meta.dev(),
    })
}

/// Calls `found` for each entry of `path`; returns how many entries could not
/// be read. Directories on a filesystem other than `dev` are skipped.
pub fn list(path: &Path, dev: u64, mut found: impl FnMut(Entry)) -> io::Result<u64> {
    let mut errors = 0;
    for entry in fs::read_dir(path)? {
        // `DirEntry::metadata` is an lstat relative to the open directory.
        let Ok((entry, meta)) = entry.and_then(|e| e.metadata().map(|m| (e, m))) else {
            errors += 1;
            continue;
        };
        let is_dir = meta.is_dir();
        if is_dir && meta.dev() != dev {
            continue;
        }
        found(Entry {
            name: entry.file_name(),
            is_dir,
            size: disk_usage(&meta),
            link: (!is_dir && meta.nlink() > 1).then(|| (meta.dev(), meta.ino())),
        });
    }
    Ok(errors)
}

/// MSIX packages only exist on Windows.
pub fn is_packaged() -> bool {
    false
}

/// Finder reveals a file (`open -R`) or opens a folder.
#[cfg(target_os = "macos")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "same signature on every platform; Linux can fail"
)]
pub fn file_manager(path: &Path, is_dir: bool) -> io::Result<Command> {
    let mut cmd = Command::new("open");
    if !is_dir {
        cmd.arg("-R");
    }
    cmd.arg(path);
    Ok(cmd)
}

/// The desktop's default file manager, on the folder (there is no portable
/// way to select a file in it).
#[cfg(not(target_os = "macos"))]
pub fn file_manager(path: &Path, is_dir: bool) -> io::Result<Command> {
    let graphical = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()));
    xdg_open(path, is_dir, graphical)
}

#[cfg(not(target_os = "macos"))]
fn xdg_open(path: &Path, is_dir: bool, graphical: bool) -> io::Result<Command> {
    // Without a desktop (SSH, a bare console), xdg-open falls back to a text
    // browser that would take over the terminal spacejunk is drawing on.
    if !graphical {
        return Err(io::Error::other("no graphical session to show it in"));
    }
    let folder = if is_dir {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let mut cmd = Command::new("xdg-open");
    cmd.arg(folder);
    Ok(cmd)
}

/// Shown under the list of folders that could not be read. macOS also
/// guards some folders (Mail, Messages...) from every app not granted Full
/// Disk Access, root included.
#[cfg(target_os = "macos")]
pub const ELEVATE_HINT: &str =
    "Run with sudo, or give your terminal Full Disk Access in System Settings";
#[cfg(not(target_os = "macos"))]
pub const ELEVATE_HINT: &str = "Run spacejunk with sudo to read them";

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn args(cmd: &Command) -> Vec<&OsStr> {
        cmd.get_args().collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finder_reveals_files_and_opens_folders() {
        let file = file_manager(Path::new("/a/b.txt"), false).unwrap();
        assert_eq!(file.get_program(), "open");
        assert_eq!(args(&file), ["-R", "/a/b.txt"]);
        assert_eq!(args(&file_manager(Path::new("/a"), true).unwrap()), ["/a"]);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn xdg_open_gets_the_folder_and_needs_a_desktop() {
        let file = xdg_open(Path::new("/a/b.txt"), false, true).unwrap();
        assert_eq!(file.get_program(), "xdg-open");
        assert_eq!(args(&file), ["/a"]);
        assert_eq!(
            args(&xdg_open(Path::new("/a/b"), true, true).unwrap()),
            ["/a/b"]
        );
        assert!(xdg_open(Path::new("/a"), true, false).is_err());
    }
}

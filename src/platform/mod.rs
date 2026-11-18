//! The only code that talks to the OS: everything above it is portable. Each
//! platform lists a directory in one pass and reports the real space every
//! entry uses on disk, and knows how to show a path in its file manager.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::thread;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{file_manager, is_packaged, list, root};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{file_manager, is_packaged, list, root};

pub struct Root {
    pub is_dir: bool,
    pub size: u64,
    /// Filesystem id; directories on another one are not descended into.
    pub dev: u64,
}

pub struct Entry {
    pub name: OsString,
    /// A directory to descend into. Symlinks, junctions and mount points of
    /// other filesystems are reported as non-directories (never followed).
    pub is_dir: bool,
    pub size: u64,
    /// Identity of a file that may have other hard links; it is charged once.
    pub link: Option<(u64, u64)>,
}

/// Shows `path` in the system file manager: a folder is opened, a file is
/// selected inside its folder where the platform can do it. Does not wait.
pub fn reveal(path: &Path, is_dir: bool) -> io::Result<()> {
    let mut cmd = file_manager(path, is_dir)?;
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("{program}: {e}")))?;
    // Reaped in the background, so it does not linger as a zombie process.
    thread::spawn(move || child.wait());
    Ok(())
}

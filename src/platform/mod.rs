//! The only code that talks to the OS filesystem API: everything above it is
//! portable. Each platform lists a directory in one pass and reports the real
//! space every entry uses on disk.

use std::ffi::OsString;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{is_packaged, list, root};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{is_packaged, list, root};

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

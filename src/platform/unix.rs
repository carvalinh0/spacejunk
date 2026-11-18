//! Linux and macOS: `read_dir` plus an lstat per entry.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

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

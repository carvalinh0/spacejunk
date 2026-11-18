//! Windows: one `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` call
//! returns a whole batch of entries with their allocated size and file id, so
//! no file is ever opened. `std::fs` cannot be used: it exposes neither the
//! allocated size (`OneDrive` placeholders report GBs but use 0 bytes) nor a
//! stable file id for hard links (the `WinSxS` folder is mostly hard links).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    APPMODEL_ERROR_NO_PACKAGE, CloseHandle, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdBothDirectoryInfo, GetFileInformationByHandleEx,
    OPEN_EXISTING,
};
use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

use super::{Entry, Root};

pub fn root(path: &Path) -> io::Result<Root> {
    let meta = fs::metadata(path)?;
    Ok(Root {
        is_dir: meta.is_dir(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        // Volumes are never crossed because mount points are reparse points.
        dev: 0,
    })
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful CreateFileW and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Absolute path with the `\\?\` prefix, so paths longer than 260 characters work.
fn long_path(path: &Path) -> io::Result<Vec<u16>> {
    let abs: Vec<u16> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    let mut out = if abs.starts_with(&wide(r"\\?\")) {
        abs
    } else if abs.starts_with(&wide(r"\\")) {
        [wide(r"\\?\UNC\"), abs[2..].to_vec()].concat()
    } else {
        [wide(r"\\?\"), abs].concat()
    };
    out.push(0);
    Ok(out)
}

fn open_dir(path: &Path) -> io::Result<Handle> {
    let name = long_path(path)?;
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(h))
}

fn entry(info: &FILE_ID_BOTH_DIR_INFO, name: &[u16]) -> Entry {
    let reparse = info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    let is_dir = info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 && !reparse;
    // FAT/exFAT report a zero id: deduplicating on it would merge every file.
    let id = info.FileId as u64;
    Entry {
        name: OsString::from_wide(name),
        is_dir,
        size: info.AllocationSize.max(0) as u64,
        link: (!is_dir && id != 0).then_some((0, id)),
    }
}

/// Calls `found` for each entry of `path`. Entry-level errors cannot happen
/// here: the batch either reads or the whole call fails.
pub fn list(path: &Path, _dev: u64, mut found: impl FnMut(Entry)) -> io::Result<u64> {
    let dir = open_dir(path)?;
    let mut buf = vec![0u64; 8 * 1024]; // 64 KiB, 8-byte aligned like the records
    let buf_len = (buf.len() * 8) as u32;
    loop {
        // SAFETY: `buf` is writable for `buf_len` bytes.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                dir.0,
                FileIdBothDirectoryInfo,
                buf.as_mut_ptr().cast(),
                buf_len,
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(0);
            }
            return Err(err);
        }
        let base = buf.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        loop {
            // SAFETY: the call wrote a chain of records into `buf`; `offset`
            // only follows `NextEntryOffset`, which stays inside what was written.
            let (info, name) = unsafe {
                #[expect(
                    clippy::cast_ptr_alignment,
                    reason = "records are 8-byte aligned by the API, and only read_unaligned/addr_of touch them"
                )]
                let rec = base.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>();
                let info = rec.read_unaligned();
                let name_ptr = ptr::addr_of!((*rec).FileName).cast::<u16>();
                let name = std::slice::from_raw_parts(name_ptr, info.FileNameLength as usize / 2);
                (info, name)
            };
            if name != [u16::from(b'.')] && name != [u16::from(b'.'), u16::from(b'.')] {
                found(entry(&info, name));
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            offset += info.NextEntryOffset as usize;
        }
    }
}

/// Running from an MSIX package (the Microsoft Store build), which Windows
/// updates by itself.
pub fn is_packaged() -> bool {
    let mut len = 0;
    // SAFETY: with a zero length the API only writes the needed length.
    let err = unsafe { GetCurrentPackageFullName(&raw mut len, ptr::null_mut()) };
    err != APPMODEL_ERROR_NO_PACKAGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_runs_tests_unpackaged() {
        assert!(!is_packaged());
    }
}

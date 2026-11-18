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
pub use unix::{ELEVATE_HINT, TRASH_NAME, file_manager, is_packaged, list, root, system_folders};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    ELEVATE_HINT, TRASH_NAME, file_manager, is_packaged, list, root, system_folders,
};

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

/// Moves `path` to the Recycle Bin / Trash, from where the user can restore
/// it. Space is only freed once the trash is emptied.
pub fn trash(path: &Path) -> io::Result<()> {
    ::trash::delete(path).map_err(|e| io::Error::other(e.to_string()))
}

/// Never moved to the trash, whatever the user's rights: a drive or
/// filesystem root, the home folder itself, and the system's own folders.
pub fn is_protected(path: &Path) -> bool {
    let Ok(path) = std::path::absolute(path) else {
        return true;
    };
    let path = key(&path);
    let inside = |folder: &Path| {
        let folder = key(folder);
        path == folder || path.starts_with(&format!("{folder}{}", std::path::MAIN_SEPARATOR))
    };
    path.is_empty()
        || !path.contains(['/', '\\'])
        || home_dir().is_some_and(|h| path == key(Path::new(&h)))
        || system_folders().iter().any(|f| inside(f))
}

/// For comparing paths: no trailing separator; Windows ignores case.
fn key(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = s.trim_end_matches(['/', '\\']);
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.to_owned()
    }
}

/// The user's home folder.
pub fn home_dir() -> Option<OsString> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn roots_home_and_system_folders_are_protected() {
        let home = PathBuf::from(home_dir().unwrap());
        assert!(is_protected(&home));
        assert!(!is_protected(&home.join("Downloads")));
        let temp = std::env::temp_dir().join("spacejunk-x");
        assert!(!is_protected(&temp));
        #[cfg(unix)]
        {
            assert!(is_protected(Path::new("/")));
            assert!(is_protected(Path::new("/usr")));
            assert!(is_protected(Path::new("/usr/lib/x")));
            assert!(!is_protected(Path::new("/usrlocal")));
        }
        #[cfg(windows)]
        {
            assert!(is_protected(Path::new(r"C:\")));
            let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
            assert!(is_protected(&system.join("System32")));
            let upper = system.to_string_lossy().to_uppercase();
            assert!(is_protected(Path::new(&upper)), "case does not matter");
            assert!(!is_protected(Path::new(r"C:\Windowsish")));
        }
    }
}

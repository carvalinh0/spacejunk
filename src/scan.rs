//! Parallel filesystem scanner that fills a tree the UI reads while it grows.
//!
//! Sizes are the real disk usage (allocated space, like `du`), hard links are
//! counted once, symlinks are never followed and other filesystems mounted
//! below the root are skipped. The OS-specific listing lives in `platform`.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use rayon::prelude::*;

use crate::platform::{self, Entry};

pub struct Node {
    pub name: Box<str>,
    pub is_dir: bool,
    /// UI state. Living in the node spares the UI a parallel id map for a
    /// tree that keeps changing shape during the scan.
    pub expanded: AtomicBool,
    /// Grows while the scan runs: listing a directory adds its entries to it
    /// and to every ancestor, so totals are live.
    size: AtomicU64,
    /// Set once, when the directory has been listed; never moves afterwards,
    /// so worker threads can keep `&Node`s into it.
    children: OnceLock<Box<[Node]>>,
    /// Moved to the trash by the user. Children cannot be taken out of the
    /// slice (see above), so a removed node stays and is skipped.
    removed: AtomicBool,
}

impl Node {
    fn new(name: Box<str>, is_dir: bool, size: u64) -> Self {
        Node {
            name,
            is_dir,
            expanded: AtomicBool::new(false),
            size: AtomicU64::new(size),
            children: OnceLock::new(),
            removed: AtomicBool::new(false),
        }
    }

    /// The node for `path`, before scanning it. Fails if `path` does not exist.
    pub fn root(path: &Path) -> io::Result<Self> {
        let info = platform::root(path)?;
        let root = Node::new(path.to_string_lossy().into(), info.is_dir, info.size);
        root.expanded.store(true, Relaxed);
        Ok(root)
    }

    pub fn size(&self) -> u64 {
        self.size.load(Relaxed)
    }

    /// Children in listing order (unsorted); empty until listed.
    pub fn children(&self) -> &[Node] {
        self.children.get().map_or(&[], |c| c)
    }

    /// A directory the scanner has not listed yet.
    pub fn is_pending(&self) -> bool {
        self.is_dir && self.children.get().is_none()
    }

    pub fn can_expand(&self) -> bool {
        self.is_pending() || self.children().iter().any(|c| !c.is_removed())
    }

    pub fn is_removed(&self) -> bool {
        self.removed.load(Relaxed)
    }

    /// The user moved this entry to the trash: it leaves the tree, and its
    /// size leaves every ancestor (`ancestors` from the root down). Only
    /// after the scan, which would otherwise keep adding to it.
    pub fn remove(&self, ancestors: &[&Node]) {
        let size = self.size.swap(0, Relaxed);
        for a in ancestors {
            a.size.fetch_sub(size, Relaxed);
        }
        self.removed.store(true, Relaxed);
    }
}

#[cfg(test)]
impl Node {
    /// A file with an exact size, for tests that must not depend on how a
    /// real filesystem allocates blocks.
    pub fn fake_file(name: &str, size: u64) -> Node {
        Node::new(name.into(), false, size)
    }

    /// A listed directory holding `children`; its size is theirs.
    pub fn fake_dir(name: &str, children: Vec<Node>) -> Node {
        let dir = Node::new(name.into(), true, children.iter().map(Node::size).sum());
        let _ = dir.children.set(children.into_boxed_slice());
        dir
    }
}

/// Live counters shared with the UI while a scan is running.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub errors: AtomicU64,
    pub cancel: AtomicBool,
    /// Folders that could not be listed, with the reason: the first
    /// `MAX_FAILED` only (`errors` keeps counting).
    pub failed: Mutex<Vec<(PathBuf, String)>>,
}

/// Enough to show what is wrong; a whole drive without permission would
/// otherwise keep one path per folder.
const MAX_FAILED: usize = 1000;

impl Progress {
    fn fail(&self, path: &Path, e: &io::Error) {
        let mut failed = self.failed.lock().unwrap();
        if failed.len() < MAX_FAILED {
            failed.push((path.to_path_buf(), e.to_string()));
        }
    }
}

/// Windows has no cheap link count, so every file id goes through this set;
/// sharding keeps 32 threads from queueing on one lock.
const SHARDS: usize = 64;

struct Ctx<'a> {
    dev: u64,
    progress: &'a Progress,
    /// Identities of possibly hard-linked files already charged.
    links: [Mutex<HashSet<(u64, u64)>>; SHARDS],
}

/// Counters for one directory, flushed to `Progress` in a single batch.
#[derive(Default)]
struct Tally {
    files: u64,
    dirs: u64,
    errors: u64,
}

impl Ctx<'_> {
    /// Size to charge for a file: a hard-linked file is charged only once.
    fn charge(&self, e: &Entry) -> u64 {
        let Some(key) = e.link else {
            return e.size;
        };
        let shard = &self.links[(key.1 as usize) % SHARDS];
        if shard.lock().unwrap().insert(key) {
            e.size
        } else {
            0
        }
    }

    // Per-entry atomics made 32 threads contend on the same cache lines;
    // one flush per directory avoids it.
    fn flush(&self, t: &Tally) {
        let p = self.progress;
        p.files.fetch_add(t.files, Relaxed);
        p.dirs.fetch_add(t.dirs, Relaxed);
        p.errors.fetch_add(t.errors, Relaxed);
    }
}

fn os_to_box(name: &OsString) -> Box<str> {
    match name.to_str() {
        Some(s) => s.into(),
        None => name.to_string_lossy().into(),
    }
}

/// Fills `root` (made by [`Node::root`] for `path`). Must run inside a rayon pool.
pub fn scan(path: &Path, root: &Node, progress: &Progress) {
    if !root.is_dir {
        progress.files.fetch_add(1, Relaxed);
        return;
    }
    let info = match platform::root(path) {
        Ok(info) => info,
        Err(e) => {
            progress.errors.fetch_add(1, Relaxed);
            progress.fail(path, &e);
            return;
        }
    };
    let ctx = Ctx {
        dev: info.dev,
        progress,
        links: std::array::from_fn(|_| Mutex::default()),
    };
    progress.dirs.fetch_add(1, Relaxed);
    scan_dir(path, root, &[], &ctx);
}

/// Lists `path`: returns its entries as nodes, plus the index and path of
/// each subdirectory to descend into.
fn list_dir(path: &Path, ctx: &Ctx) -> (Vec<Node>, Vec<(usize, PathBuf)>) {
    let (mut nodes, mut subdirs, mut tally) = (Vec::new(), Vec::new(), Tally::default());
    let listed = platform::list(path, ctx.dev, |e| {
        let name = os_to_box(&e.name);
        if e.is_dir {
            tally.dirs += 1;
            subdirs.push((nodes.len(), path.join(&e.name)));
            nodes.push(Node::new(name, true, e.size));
        } else {
            tally.files += 1;
            nodes.push(Node::new(name, false, ctx.charge(&e)));
        }
    });
    // Entry-level errors (an entry gone between listing and lstat) are
    // only counted; a folder that cannot be listed is also named.
    tally.errors = listed.unwrap_or_else(|e| {
        ctx.progress.fail(path, &e);
        1
    });
    ctx.flush(&tally);
    (nodes, subdirs)
}

fn scan_dir(path: &Path, node: &Node, ancestors: &[&Node], ctx: &Ctx) {
    if ctx.progress.cancel.load(Relaxed) {
        return;
    }
    let (nodes, subdirs) = list_dir(path, ctx);
    let added: u64 = nodes.iter().map(Node::size).sum();
    for n in ancestors.iter().copied().chain([node]) {
        n.size.fetch_add(added, Relaxed);
    }
    let children = node.children.get_or_init(|| nodes.into_boxed_slice());
    if subdirs.is_empty() {
        return;
    }
    let mut chain = Vec::with_capacity(ancestors.len() + 1);
    chain.extend_from_slice(ancestors);
    chain.push(node);
    subdirs
        .into_par_iter()
        .for_each(|(i, sub)| scan_dir(&sub, &children[i], &chain, ctx));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("spacejunk-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run(path: &Path) -> Node {
        run_with(path, &Progress::default())
    }

    fn run_with(path: &Path, progress: &Progress) -> Node {
        let root = Node::root(path).unwrap();
        scan(path, &root, progress);
        root
    }

    fn child<'a>(n: &'a Node, name: &str) -> &'a Node {
        n.children().iter().find(|c| &*c.name == name).unwrap()
    }

    fn sum_children(n: &Node) -> u64 {
        n.children().iter().map(Node::size).sum()
    }

    /// Portable: holds whatever a directory's own size is on this platform.
    #[test]
    fn parents_include_everything_below_them() {
        let t = TempDir::new("tree");
        let root = &t.0;
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/data"), vec![1u8; 256 * 1024]).unwrap();
        fs::write(root.join("small"), vec![1u8; 4096]).unwrap();

        let node = run(root);
        let a = child(&node, "a");
        let b = child(a, "b");
        assert!(child(b, "data").size() >= 256 * 1024);
        for dir in [&node, a, b] {
            let own = dir.size() - sum_children(dir);
            assert!(own < 1024 * 1024, "{}: own size {own}", dir.name);
            assert!(!dir.is_pending());
        }
    }

    #[test]
    fn hardlinks_counted_once() {
        let t = TempDir::new("hardlink");
        let root = &t.0;
        fs::write(root.join("a"), vec![1u8; 128 * 1024]).unwrap();
        fs::hard_link(root.join("a"), root.join("b")).unwrap();

        let node = run(root);
        let mut sizes = [child(&node, "a").size(), child(&node, "b").size()];
        sizes.sort_unstable();
        assert_eq!(sizes[0], 0, "second link is free");
        assert!(sizes[1] >= 128 * 1024);
    }

    #[test]
    fn cancel_leaves_directories_pending() {
        let t = TempDir::new("cancel");
        fs::create_dir(t.0.join("d")).unwrap();
        let root = Node::root(&t.0).unwrap();
        let p = Progress::default();
        p.cancel.store(true, Relaxed);
        scan(&t.0, &root, &p);
        assert!(root.is_pending());
        assert!(root.children().is_empty());
    }

    /// Portable stand-in for an unreadable folder: it vanishes before the
    /// scan lists it.
    #[test]
    fn folders_that_cannot_be_listed_are_kept_with_the_reason() {
        let t = TempDir::new("vanished");
        let gone = t.0.join("gone");
        fs::create_dir(&gone).unwrap();
        let root = Node::root(&gone).unwrap();
        fs::remove_dir(&gone).unwrap();
        let p = Progress::default();
        scan(&gone, &root, &p);
        assert_eq!(p.errors.load(Relaxed), 1);
        let failed = p.failed.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, gone);
        assert_ne!(failed[0].1, "");
    }

    #[test]
    fn missing_root_is_an_error() {
        assert!(Node::root(Path::new("/definitely/not/here")).is_err());
    }

    /// Exact numbers against `st_blocks`, the same source `du` uses.
    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::MetadataExt;

        fn usage(p: &Path) -> u64 {
            fs::symlink_metadata(p).unwrap().blocks() * 512
        }

        #[test]
        fn sizes_match_du_at_every_level() {
            let t = TempDir::new("sizes");
            let root = &t.0;
            fs::create_dir_all(root.join("a/b")).unwrap();
            fs::write(root.join("a/b/data"), vec![1u8; 256 * 1024]).unwrap();
            fs::write(root.join("small"), vec![1u8; 4096]).unwrap();

            let node = run(root);
            let b = usage(&root.join("a/b")) + usage(&root.join("a/b/data"));
            let a = usage(&root.join("a")) + b;
            assert_eq!(child(child(&node, "a"), "b").size(), b);
            assert_eq!(child(&node, "a").size(), a);
            assert_eq!(node.size(), usage(root) + a + usage(&root.join("small")));
        }

        #[test]
        fn unreadable_folders_are_listed() {
            use std::os::unix::fs::PermissionsExt;
            let t = TempDir::new("locked");
            let locked = t.0.join("locked");
            fs::create_dir(&locked).unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
            let readable_anyway = fs::read_dir(&locked).is_ok(); // running as root
            let p = Progress::default();
            run_with(&t.0, &p);
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            if readable_anyway {
                return;
            }
            let failed = p.failed.lock().unwrap();
            assert_eq!(failed.iter().map(|f| &f.0).collect::<Vec<_>>(), [&locked]);
        }

        #[test]
        fn symlinks_not_followed() {
            let t = TempDir::new("symlink");
            let root = &t.0;
            fs::create_dir(root.join("inner")).unwrap();
            fs::write(root.join("target"), vec![1u8; 128 * 1024]).unwrap();
            std::os::unix::fs::symlink(root, root.join("inner/loop")).unwrap();

            let node = run(root);
            let link = child(child(&node, "inner"), "loop");
            assert!(!link.is_dir);
            assert_eq!(link.size(), usage(&root.join("inner/loop")));
        }
    }
}

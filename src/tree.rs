//! The flattened, size-sorted list of rows the tree view shows.
//!
//! Rows point at nodes by position (parent row + index among its children)
//! instead of borrowing them, so the view can own the list next to the tree
//! the scanner is still filling. Rebuilding is cheap: only expanded nodes
//! are visited.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::Ordering::Relaxed;

use crate::junk;
use crate::scan::Node;

pub const NONE: u32 = u32::MAX;

#[derive(Clone, Copy)]
pub struct Row {
    pub parent: u32,
    pub child: u32,
    pub depth: u16,
}

/// Depth-first rows of the expanded nodes, children largest first. Also
/// returns the row now holding `keep`, so the selection can follow a node
/// whose position changed because sizes grew.
pub fn build(root: &Node, keep: *const Node) -> (Vec<Row>, Option<usize>) {
    let mut b = Builder {
        rows: Vec::new(),
        keep,
        found: None,
    };
    b.push(
        root,
        Row {
            parent: NONE,
            child: 0,
            depth: 0,
        },
    );
    (b.rows, b.found)
}

struct Builder {
    rows: Vec<Row>,
    keep: *const Node,
    found: Option<usize>,
}

impl Builder {
    fn push(&mut self, node: &Node, row: Row) {
        let me = self.rows.len();
        if ptr::eq(node, self.keep) {
            self.found = Some(me);
        }
        self.rows.push(row);
        if !node.expanded.load(Relaxed) {
            return;
        }
        let kids = node.children();
        // Sizes change under our feet during a scan: snapshot them, and break
        // ties by index so equal-sized rows do not swap between frames.
        let mut order: Vec<(u64, u32)> = kids
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_removed())
            .map(|(i, c)| (c.size(), i as u32))
            .collect();
        order.sort_unstable_by_key(|&(size, i)| (Reverse(size), i));
        for (_, i) in order {
            let child = Row {
                parent: me as u32,
                child: i,
                depth: row.depth.saturating_add(1),
            };
            self.push(&kids[i as usize], child);
        }
    }
}

pub fn node<'a>(root: &'a Node, rows: &[Row], i: usize) -> &'a Node {
    let r = rows[i];
    if r.parent == NONE {
        root
    } else {
        &node(root, rows, r.parent as usize).children()[r.child as usize]
    }
}

pub fn path_of(root: &Node, rows: &[Row], mut i: usize) -> PathBuf {
    let mut parts = Vec::new();
    while rows[i].parent != NONE {
        parts.push(&*node(root, rows, i).name);
        i = rows[i].parent as usize;
    }
    let mut p = PathBuf::from(&*root.name);
    p.extend(parts.iter().rev());
    p
}

/// One line of a flat list: the largest files, or the junk folders.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Listed {
    pub size: u64,
    pub path: PathBuf,
    pub is_dir: bool,
    /// What it is, for junk ("npm packages"); empty otherwise.
    pub tag: &'static str,
}

/// The `n` largest files below `root`, largest first.
pub fn largest_files(root: &Node, n: usize) -> Vec<Listed> {
    top(root, n, &|node, _, _| ((!node.is_dir).then_some(""), true))
}

/// The `n` largest junk folders below `root` (see `junk`), largest first.
/// A junk folder is listed whole, never what is inside it.
pub fn junk_folders(root: &Node, n: usize) -> Vec<Listed> {
    top(
        root,
        n,
        &|node, parent, path| match junk::kind(node, parent, path) {
            Some(what) => (Some(what), false),
            None => (None, true),
        },
    )
}

/// The `n` largest files and folders whose name contains `query`, ignoring
/// case, largest first. A matching folder is searched too.
pub fn search(root: &Node, query: &str, n: usize) -> Vec<Listed> {
    if query.is_empty() {
        return Vec::new();
    }
    let query = query.to_lowercase();
    top(root, n, &|node, _, _| {
        (
            contains_ignoring_case(&node.name, &query).then_some(""),
            true,
        )
    })
}

/// `query` is lowercase already. ASCII names, nearly all of them, are
/// compared without allocating: a search runs on every key typed.
fn contains_ignoring_case(name: &str, query: &str) -> bool {
    if name.is_ascii() && query.is_ascii() {
        let query = query.as_bytes();
        name.as_bytes()
            .windows(query.len())
            .any(|w| w.eq_ignore_ascii_case(query))
    } else {
        name.to_lowercase().contains(query)
    }
}

/// For each entry (with its parent and path), `pick` says whether to list it
/// (with its tag) and whether to look inside it.
type Pick<'a> = &'a dyn Fn(&Node, &Node, &Path) -> (Option<&'static str>, bool);

/// The `n` largest entries `pick` lists. Walks the whole tree: call it on
/// demand, not on every frame.
fn top(root: &Node, n: usize, pick: Pick) -> Vec<Listed> {
    let mut heap = BinaryHeap::with_capacity(n + 1);
    walk(root, &mut PathBuf::from(&*root.name), n, pick, &mut heap);
    let mut listed: Vec<Listed> = heap.into_iter().map(|Reverse(l)| l).collect();
    listed.sort_unstable_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    listed
}

/// A min-heap of the `n` largest so far: millions of entries go by, and only
/// one larger than the smallest kept builds its path.
fn walk(
    dir: &Node,
    path: &mut PathBuf,
    n: usize,
    pick: Pick,
    heap: &mut BinaryHeap<Reverse<Listed>>,
) {
    for child in dir.children().iter().filter(|c| !c.is_removed()) {
        path.push(&*child.name);
        let size = child.size();
        let (tag, look_inside) = pick(child, dir, path);
        if let Some(tag) = tag
            && (heap.len() < n || heap.peek().is_some_and(|Reverse(min)| size > min.size))
        {
            if heap.len() == n {
                heap.pop();
            }
            let is_dir = child.is_dir;
            heap.push(Reverse(Listed {
                size,
                path: path.clone(),
                is_dir,
                tag,
            }));
        }
        if child.is_dir && look_inside {
            walk(child, path, n, pick, heap);
        }
        path.pop();
    }
}

/// The nodes from `root` down to the one at `path`, if it is in the tree.
pub fn find<'a>(root: &'a Node, path: &Path) -> Option<Vec<&'a Node>> {
    let rel = path.strip_prefix(&*root.name).ok()?;
    let mut chain = vec![root];
    for part in rel.components() {
        let name = part.as_os_str().to_str()?;
        let parent = chain.last()?;
        let child = parent
            .children()
            .iter()
            .find(|c| &*c.name == name && !c.is_removed())?;
        chain.push(child);
    }
    Some(chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn largest_files_across_folders_largest_first() {
        let root = Node::fake_dir(
            "/r",
            vec![
                Node::fake_file("small", 1),
                Node::fake_dir(
                    "a",
                    vec![
                        Node::fake_file("big", 50),
                        Node::fake_dir("b", vec![Node::fake_file("mid", 20)]),
                    ],
                ),
                Node::fake_file("top", 99),
                Node::fake_dir("empty", Vec::new()),
            ],
        );
        let files = largest_files(&root, 3);
        let expected = [(99, "/r/top"), (50, "/r/a/big"), (20, "/r/a/b/mid")];
        assert_eq!(files.len(), expected.len());
        for (file, (want_size, want_path)) in files.iter().zip(expected) {
            assert_eq!(
                (file.size, &file.path),
                (want_size, &PathBuf::from(want_path))
            );
        }
        assert_eq!(largest_files(&root, 0), Vec::new());
        assert_eq!(largest_files(&root, 10).len(), 4);
    }

    #[test]
    fn a_removed_entry_leaves_the_rows_the_files_and_its_ancestors_sizes() {
        let root = Node::fake_dir(
            "/r",
            vec![
                Node::fake_dir(
                    "a",
                    vec![Node::fake_file("big", 50), Node::fake_file("x", 5)],
                ),
                Node::fake_file("small", 1),
            ],
        );
        let path = PathBuf::from("/r").join("a").join("big");
        let chain = find(&root, &path).expect("found");
        let names: Vec<&str> = chain.iter().map(|n| &*n.name).collect();
        assert_eq!(names, ["/r", "a", "big"]);
        let (big, ancestors) = chain.split_last().unwrap();
        big.remove(ancestors);

        assert_eq!(root.size(), 6);
        assert_eq!(ancestors[1].size(), 5);
        assert!(find(&root, &path).is_none());
        root.expanded.store(true, Relaxed);
        root.children()[0].expanded.store(true, Relaxed);
        let (rows, _) = build(&root, ptr::null());
        let shown: Vec<String> = (0..rows.len())
            .map(|i| node(&root, &rows, i).name.to_string())
            .collect();
        assert_eq!(shown, ["/r", "a", "x", "small"]);
        let files: Vec<u64> = largest_files(&root, 10).iter().map(|f| f.size).collect();
        assert_eq!(files, [5, 1]);
    }

    #[test]
    fn junk_folders_are_listed_whole_largest_first() {
        let nested = Node::fake_dir("node_modules", vec![Node::fake_file("x", 40)]);
        let deps = Node::fake_dir(
            "node_modules",
            vec![
                nested,
                Node::fake_file("package.json", 0),
                Node::fake_file("yarn.lock", 0),
            ],
        );
        let target = Node::fake_dir(
            "target",
            vec![
                Node::fake_file("CACHEDIR.TAG", 1),
                Node::fake_file("app", 60),
            ],
        );
        let root = Node::fake_dir(
            "/r",
            vec![
                Node::fake_dir(
                    "web",
                    vec![
                        deps,
                        Node::fake_file("package.json", 1),
                        Node::fake_file("package-lock.json", 4),
                    ],
                ),
                Node::fake_dir("rust", vec![target]),
                Node::fake_dir("src", vec![Node::fake_file("main.rs", 9)]),
            ],
        );
        let junk = junk_folders(&root, 10);
        let found: Vec<(u64, &str)> = junk.iter().map(|j| (j.size, j.tag)).collect();
        assert_eq!(found, [(61, "cache (CACHEDIR.TAG)"), (40, "npm packages")]);
        assert_eq!(
            junk[1].path,
            PathBuf::from("/r/web/node_modules"),
            "not the nested one"
        );
    }

    #[test]
    fn search_matches_names_ignoring_case_folders_and_files() {
        let root = Node::fake_dir(
            "/r",
            vec![
                Node::fake_dir("Photos", vec![Node::fake_file("photo.JPG", 30)]),
                Node::fake_dir("music", vec![Node::fake_file("song.mp3", 50)]),
                Node::fake_dir("Ação", vec![Node::fake_file("ÇÃO.txt", 2)]),
            ],
        );
        let names = |q: &str| -> Vec<(String, bool)> {
            search(&root, q, 10)
                .iter()
                .map(|l| {
                    (
                        l.path.file_name().unwrap().to_string_lossy().into_owned(),
                        l.is_dir,
                    )
                })
                .collect()
        };
        let photo = |name: &str, dir| (name.to_owned(), dir);
        assert_eq!(
            names("PHOTO"),
            [photo("Photos", true), photo("photo.JPG", false)]
        );
        assert_eq!(names("ção"), [photo("Ação", true), photo("ÇÃO.txt", false)]);
        assert_eq!(names("zzz"), []);
        assert_eq!(names(""), [], "an empty search lists nothing");
    }

    #[test]
    fn rows_sorted_largest_first_and_follow_expansion() {
        let dir = std::env::temp_dir().join(format!("spacejunk-rows-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("big")).unwrap();
        fs::write(dir.join("big/data"), vec![1u8; 256 * 1024]).unwrap();
        fs::write(dir.join("small"), vec![1u8; 4096]).unwrap();
        fs::write(dir.join("mid"), vec![1u8; 64 * 1024]).unwrap();
        let root = Node::root(&dir).unwrap();
        crate::scan::scan(&dir, &root, &crate::scan::Progress::default());
        fs::remove_dir_all(&dir).unwrap();

        let names = |rows: &[Row]| -> Vec<String> {
            (0..rows.len())
                .map(|i| node(&root, rows, i).name.to_string())
                .collect()
        };
        let (rows, _) = build(&root, ptr::null());
        assert_eq!(names(&rows)[1..], ["big", "mid", "small"]);

        let big = node(&root, &rows, 1);
        big.expanded.store(true, Relaxed);
        let (rows, found) = build(&root, big);
        assert_eq!(names(&rows)[1..], ["big", "data", "mid", "small"]);
        assert_eq!(found, Some(1));
        assert_eq!(rows[2].depth, 2);
        assert_eq!(path_of(&root, &rows, 2), dir.join("big/data"));
    }
}

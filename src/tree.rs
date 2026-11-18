//! The flattened, size-sorted list of rows the tree view shows.
//!
//! Rows point at nodes by position (parent row + index among its children)
//! instead of borrowing them, so the view can own the list next to the tree
//! the scanner is still filling. Rebuilding is cheap: only expanded nodes
//! are visited.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::Ordering::Relaxed;

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

/// The `n` largest files below `root` with their paths, largest first.
/// Walks the whole tree: call it on demand, not on every frame.
pub fn largest_files(root: &Node, n: usize) -> Vec<(u64, PathBuf)> {
    let mut heap = BinaryHeap::with_capacity(n + 1);
    collect_files(root, &mut PathBuf::from(&*root.name), n, &mut heap);
    let mut files: Vec<_> = heap.into_iter().map(|Reverse(f)| f).collect();
    files.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files
}

/// A min-heap of the `n` largest files so far: millions of files go by, and
/// only one larger than the smallest kept builds its path.
fn collect_files(
    dir: &Node,
    path: &mut PathBuf,
    n: usize,
    heap: &mut BinaryHeap<Reverse<(u64, PathBuf)>>,
) {
    for child in dir.children() {
        path.push(&*child.name);
        let size = child.size();
        if child.is_dir {
            collect_files(child, path, n, heap);
        } else if heap.len() < n {
            heap.push(Reverse((size, path.clone())));
        } else if heap.peek().is_some_and(|Reverse((min, _))| size > *min) {
            heap.pop();
            heap.push(Reverse((size, path.clone())));
        }
        path.pop();
    }
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
        for ((size, path), (want_size, want_path)) in files.iter().zip(expected) {
            assert_eq!((*size, path), (want_size, &PathBuf::from(want_path)));
        }
        assert_eq!(largest_files(&root, 0), Vec::<(u64, PathBuf)>::new());
        assert_eq!(largest_files(&root, 10).len(), 4);
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

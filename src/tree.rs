//! The flattened, size-sorted list of rows the tree view shows.
//!
//! Rows point at nodes by position (parent row + index among its children)
//! instead of borrowing them, so the view can own the list next to the tree
//! the scanner is still filling. Rebuilding is cheap: only expanded nodes
//! are visited.

use std::cmp::Reverse;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

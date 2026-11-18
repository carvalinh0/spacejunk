//! Application state and key handling.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rayon::ThreadPool;
use sysinfo::Disks;

use crate::scan::{self, Node, Progress};
use crate::tree::{self, NONE, Row};
use crate::update;

pub struct DiskEntry {
    pub mount: PathBuf,
    pub device: String,
    pub fs: String,
    pub total: u64,
    pub used: u64,
}

/// A scanned (or still scanning) directory tree on screen.
pub struct TreeView {
    pub path: PathBuf,
    pub root: Arc<Node>,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub offset: usize,
    pub page: usize,
    pub progress: Arc<Progress>,
    started: Instant,
    finished: Option<Duration>,
    job: Option<JoinHandle<()>>,
}

pub enum Screen {
    Disks,
    Tree(TreeView),
}

pub struct App {
    pub screen: Screen,
    pub disks: Vec<DiskEntry>,
    pub disk_selected: usize,
    /// Text of the "scan a path" prompt, while it is open.
    pub input: Option<String>,
    pub message: Option<String>,
    /// A newer version published on crates.io, once the background check answers.
    pub update: Option<String>,
    pub quit: bool,
    pool: Arc<ThreadPool>,
    update_rx: Receiver<String>,
}

pub fn build_pool() -> ThreadPool {
    // Directory scanning is syscall/IO bound: oversubscribing the CPUs keeps
    // the disk queue full while some threads block in the kernel.
    // Measured with hyperfine on 16 cores: 20-50 threads are equally fast;
    // 10 threads were 12% slower, 8 threads 27%, and 60 gained nothing.
    let threads = thread::available_parallelism().map_or(4, std::num::NonZero::get) * 2;
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.clamp(4, 64))
        .stack_size(16 * 1024 * 1024) // deep directory trees recurse
        .thread_name(|i| format!("scan-{i}"))
        .build()
        .expect("failed to build thread pool")
}

pub fn list_disks() -> Vec<DiskEntry> {
    let Ok(disks) = Disks::new_with_refreshed_list() else {
        return Vec::new();
    };
    let mut out: Vec<DiskEntry> = disks
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| DiskEntry {
            mount: d.mount_point().to_path_buf(),
            device: unescape_mount(&d.name().to_string_lossy()),
            fs: d.file_system().to_string_lossy().into_owned(),
            total: d.total_space(),
            used: d.total_space().saturating_sub(d.available_space()),
        })
        .filter(|d| !HIDDEN_FS.contains(&d.fs.as_str()))
        .collect();
    // Sorted by mount, a device's top-most mount comes first; later ones are
    // bind mounts of the same data (e.g. /var/lib/docker on the root disk).
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    let mut seen = HashSet::new();
    // Windows volumes without a label have an empty name: never merge those.
    out.retain(|d| matches!(d.device.as_str(), "" | "none") || seen.insert(d.device.clone()));
    out.dedup_by(|a, b| a.mount == b.mount);
    out
}

/// Virtual filesystems that are not worth analyzing.
const HIDDEN_FS: [&str; 6] = [
    "overlay", "squashfs", "tmpfs", "devtmpfs", "ramfs", "efivarfs",
];

/// `/proc/mounts` escapes space, tab, newline and backslash as `\ooo` octal.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..=i + 3].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The shell would expand `~`; a path typed in the TUI does not go through one.
fn expand_tilde(s: &str) -> PathBuf {
    match (s.strip_prefix('~'), home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            PathBuf::from(format!("{}{rest}", home.to_string_lossy()))
        }
        _ => PathBuf::from(s),
    }
}

fn home_dir() -> Option<std::ffi::OsString> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
}

impl App {
    pub fn new(start: Option<PathBuf>) -> Self {
        let mut app = App {
            screen: Screen::Disks,
            disks: list_disks(),
            disk_selected: 0,
            input: None,
            message: None,
            update: None,
            quit: false,
            pool: Arc::new(build_pool()),
            update_rx: update::spawn_check(),
        };
        if let Some(path) = start {
            app.start_scan(&path);
        }
        app
    }

    pub fn is_scanning(&self) -> bool {
        matches!(&self.screen, Screen::Tree(v) if v.is_scanning())
    }

    pub fn start_scan(&mut self, path: &Path) {
        self.stop_scan();
        match TreeView::start(path, self.pool.clone()) {
            Ok(view) => {
                self.message = None;
                self.screen = Screen::Tree(view);
            }
            Err(e) => self.message = Some(format!("{}: {e}", path.display())),
        }
    }

    fn stop_scan(&self) {
        if let Screen::Tree(view) = &self.screen {
            view.progress.cancel.store(true, Relaxed);
        }
    }

    pub fn tick(&mut self) {
        if let Screen::Tree(view) = &mut self.screen {
            view.tick();
        }
        if let Ok(version) = self.update_rx.try_recv() {
            self.update = Some(version);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.stop_scan();
            self.quit = true;
            return;
        }
        if self.input.is_some() {
            self.on_key_input(key.code);
            return;
        }
        match self.screen {
            Screen::Disks => self.on_key_disks(key.code),
            Screen::Tree(_) => self.on_key_tree(key.code),
        }
    }

    fn on_key_input(&mut self, code: KeyCode) {
        let Some(text) = &mut self.input else {
            return;
        };
        match code {
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Esc => self.input = None,
            KeyCode::Enter => {
                let path = expand_tilde(text.trim());
                self.input = None;
                self.start_scan(&path);
            }
            _ => {}
        }
    }

    fn on_key_tree(&mut self, code: KeyCode) {
        let Screen::Tree(view) = &mut self.screen else {
            return;
        };
        match code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc | KeyCode::Char('b') => {
                self.stop_scan();
                self.disks = list_disks();
                self.screen = Screen::Disks;
            }
            KeyCode::Char('r') => {
                let path = view.path.clone();
                self.start_scan(&path);
            }
            code => view.on_key(code),
        }
    }

    fn on_key_disks(&mut self, code: KeyCode) {
        let n = self.disks.len();
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => {
                self.disk_selected = self.disk_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.disk_selected = (self.disk_selected + 1).min(n.saturating_sub(1));
            }
            KeyCode::Char('r') => {
                self.disks = list_disks();
                self.disk_selected = self.disk_selected.min(self.disks.len().saturating_sub(1));
            }
            KeyCode::Char('p' | '/') => {
                // Prefilled so a relative target is one subfolder name away.
                let mut cwd = std::env::current_dir()
                    .unwrap_or_default()
                    .display()
                    .to_string();
                if !cwd.ends_with(std::path::MAIN_SEPARATOR) {
                    cwd.push(std::path::MAIN_SEPARATOR);
                }
                self.input = Some(cwd);
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(d) = self.disks.get(self.disk_selected) {
                    let mount = d.mount.clone();
                    self.start_scan(&mount);
                }
            }
            _ => {}
        }
    }
}

impl TreeView {
    /// Opens the view at once and fills it from a background scan.
    fn start(path: &Path, pool: Arc<ThreadPool>) -> io::Result<Self> {
        let root = Arc::new(Node::root(path)?);
        let progress = Arc::new(Progress::default());
        let (r, p, scan_path) = (root.clone(), progress.clone(), path.to_path_buf());
        let job = thread::spawn(move || pool.install(|| scan::scan(&scan_path, &r, &p)));
        let mut view = TreeView {
            path: path.to_path_buf(),
            root,
            rows: Vec::new(),
            selected: 0,
            offset: 0,
            page: 10,
            progress,
            started: Instant::now(),
            finished: None,
            job: Some(job),
        };
        view.refresh();
        Ok(view)
    }

    pub fn is_scanning(&self) -> bool {
        self.job.is_some()
    }

    pub fn elapsed(&self) -> Duration {
        self.finished.unwrap_or_else(|| self.started.elapsed())
    }

    /// While scanning, re-sorts the rows so the view tracks the growing sizes.
    fn tick(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        if job.is_finished() {
            self.job = None;
            self.finished = Some(self.started.elapsed());
        }
        self.refresh();
    }

    pub fn selected_node(&self) -> &Node {
        tree::node(&self.root, &self.rows, self.selected)
    }

    fn refresh(&mut self) {
        let keep: *const Node = if self.rows.is_empty() {
            Arc::as_ptr(&self.root)
        } else {
            self.selected_node()
        };
        let (rows, found) = tree::build(&self.root, keep);
        self.rows = rows;
        self.selected = found.unwrap_or(self.selected).min(self.rows.len() - 1);
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    fn on_key(&mut self, code: KeyCode) {
        let page = self.page.max(1) as isize;
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = self.rows.len() - 1,
            KeyCode::Right | KeyCode::Char('l') => self.open(),
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle(),
            KeyCode::Left | KeyCode::Char('h') => self.close(),
            _ => {}
        }
    }

    fn is_open(&self) -> bool {
        let n = self.selected_node();
        n.expanded.load(Relaxed) && n.can_expand()
    }

    fn set_open(&mut self, open: bool) {
        self.selected_node().expanded.store(open, Relaxed);
        self.refresh();
    }

    /// Opens the selected folder; on an already open one, steps into it.
    fn open(&mut self) {
        if self.is_open() {
            self.move_by(1);
        } else if self.selected_node().can_expand() {
            self.set_open(true);
        }
    }

    fn toggle(&mut self) {
        if self.selected_node().can_expand() {
            let open = self.is_open();
            self.set_open(!open);
        }
    }

    /// Closes the selected folder; on a closed one, jumps to its parent.
    fn close(&mut self) {
        let parent = self.rows[self.selected].parent;
        if self.is_open() {
            self.set_open(false);
        } else if parent != NONE {
            self.selected = parent as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn unescapes_octal_in_mount_names() {
        assert_eq!(unescape_mount(r"C:\134"), r"C:\");
        assert_eq!(unescape_mount(r"a\040b"), "a b");
        assert_eq!(unescape_mount(r"plain\x"), r"plain\x");
        assert_eq!(unescape_mount(r"end\04"), r"end\04");
    }

    #[test]
    fn tilde_expands_to_home_only_as_a_prefix() {
        let home = home_dir().unwrap().to_string_lossy().into_owned();
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
        assert_eq!(expand_tilde("~/x"), PathBuf::from(format!("{home}/x")));
        assert_eq!(expand_tilde("~user"), PathBuf::from("~user"));
        assert_eq!(expand_tilde("/a/~"), PathBuf::from("/a/~"));
    }

    /// `root/` (open) > `a/` (closed, holds `a1`), then the smaller file `b`.
    fn view(tag: &str) -> TreeView {
        let dir = std::env::temp_dir().join(format!("spacejunk-app-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::write(dir.join("a/a1"), vec![1u8; 256 * 1024]).unwrap();
        fs::write(dir.join("b"), vec![1u8; 4096]).unwrap();
        let mut v = TreeView::start(&dir, Arc::new(build_pool())).unwrap();
        v.job.take().unwrap().join().unwrap();
        fs::remove_dir_all(&dir).unwrap();
        v.refresh();
        v
    }

    fn selected_name(v: &TreeView) -> &str {
        &v.selected_node().name
    }

    #[test]
    fn right_opens_then_steps_in_left_closes_then_goes_up() {
        let mut v = view("nav");
        v.on_key(KeyCode::Down);
        assert_eq!(selected_name(&v), "a");
        v.on_key(KeyCode::Right);
        assert_eq!(v.rows.len(), 4, "a opened");
        v.on_key(KeyCode::Right);
        assert_eq!(selected_name(&v), "a1", "stepped into a");
        v.on_key(KeyCode::Left);
        assert_eq!(selected_name(&v), "a", "leaf jumps to parent");
        v.on_key(KeyCode::Left);
        assert_eq!(v.rows.len(), 3, "a closed");
        assert_eq!(selected_name(&v), "a");
    }

    #[test]
    fn movement_is_clamped() {
        let mut v = view("clamp");
        v.on_key(KeyCode::Up);
        assert_eq!(v.selected, 0);
        v.on_key(KeyCode::PageDown);
        assert_eq!(selected_name(&v), "b");
        v.on_key(KeyCode::Enter);
        assert_eq!(v.rows.len(), 3, "a file does not toggle");
    }

    #[test]
    fn selection_follows_its_node_when_rows_reorder() {
        let mut v = view("follow");
        v.on_key(KeyCode::End);
        assert_eq!(selected_name(&v), "b");
        let a = v.root.children().iter().find(|c| &*c.name == "a").unwrap();
        a.expanded.store(true, Relaxed); // `a` gains a row above `b`
        v.refresh();
        assert_eq!(selected_name(&v), "b");
        assert_eq!(v.selected, 3);
    }
}

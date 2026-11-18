//! Application state and key handling.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use rayon::ThreadPool;
use sysinfo::Disks;

use crate::platform;
use crate::scan::{self, Node, Progress};
use crate::tree::{self, Listed, NONE, Row};
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
    /// A flat list shown instead of the tree while open (`f`, `e`).
    pub panel: Option<Box<Panel>>,
    started: Instant,
    finished: Option<Duration>,
    job: Option<JoinHandle<()>>,
}

/// How many lines a list built from the tree keeps.
const LIST_LIMIT: usize = 1000;
/// Such a list walks the whole tree, so a running scan refreshes it this
/// often, not every frame.
const LIST_REFRESH: Duration = Duration::from_secs(1);

pub enum Panel {
    /// A list built from the tree: the largest files (`f`) or the junk (`c`).
    List(ListView),
    /// The folders that could not be read (`e`): `Progress::failed`, which
    /// grows during the scan, so only the selection lives here.
    Errors(ListNav),
}

/// Selection and scrolling of a flat list.
pub struct ListNav {
    pub selected: usize,
    pub offset: usize,
    pub page: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    LargestFiles,
    Junk,
}

pub struct ListView {
    pub kind: ListKind,
    /// Largest first.
    pub items: Vec<Listed>,
    pub nav: ListNav,
    built: Instant,
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
    /// An error, in red in the title.
    pub message: Option<String>,
    /// What just worked (a move to the trash), until the next key.
    pub notice: Option<String>,
    /// Waiting for `y` to move this to the trash.
    pub confirm: Option<Removal>,
    /// A newer version published on crates.io, once the background check answers.
    pub update: Option<String>,
    pub quit: bool,
    /// Where the current list's lines were drawn, for mouse clicks.
    pub list_area: Rect,
    /// First disk shown, when the disk list scrolls.
    pub disk_offset: usize,
    pool: Arc<ThreadPool>,
    update_rx: Receiver<String>,
    /// `platform::trash`; tests put a fake here that only records the path.
    trash: fn(&Path) -> io::Result<()>,
}

pub struct Removal {
    pub path: PathBuf,
    pub size: u64,
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
    match (s.strip_prefix('~'), platform::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            PathBuf::from(format!("{}{rest}", home.to_string_lossy()))
        }
        _ => PathBuf::from(s),
    }
}

impl App {
    pub fn new(start: Option<PathBuf>) -> Self {
        let mut app = App {
            screen: Screen::Disks,
            disks: list_disks(),
            disk_selected: 0,
            input: None,
            message: None,
            notice: None,
            confirm: None,
            update: None,
            quit: false,
            list_area: Rect::default(),
            disk_offset: 0,
            pool: Arc::new(build_pool()),
            update_rx: update::spawn_check(),
            trash: platform::trash,
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
        self.notice = None;
        if let Some(removal) = self.confirm.take() {
            if key.code == KeyCode::Char('y') {
                self.remove(&removal);
            }
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

    /// The selection follows the pointer and the wheel; a click on the
    /// selected line does what Enter does (open a folder, scan a disk). In
    /// terminals that report no pointer moves, the first click selects.
    pub fn on_mouse(&mut self, m: MouseEvent) {
        if self.input.is_some() || self.confirm.is_some() {
            return;
        }
        let key = |code: KeyCode| KeyEvent::from(code);
        match m.kind {
            MouseEventKind::ScrollDown => (0..3).for_each(|_| self.on_key(key(KeyCode::Down))),
            MouseEventKind::ScrollUp => (0..3).for_each(|_| self.on_key(key(KeyCode::Up))),
            MouseEventKind::Down(MouseButton::Left) => {
                if self.point_at(m.column, m.row) == Some(true) {
                    self.on_key(key(KeyCode::Enter));
                }
            }
            MouseEventKind::Moved => {
                self.point_at(m.column, m.row);
            }
            _ => {}
        }
    }

    /// Selects the line under the pointer, if there is one; says whether it
    /// was selected already.
    fn point_at(&mut self, x: u16, y: u16) -> Option<bool> {
        if !self.list_area.contains(Position { x, y }) {
            return None;
        }
        let line = usize::from(y - self.list_area.y);
        let (selected, offset, len) = self.current_list();
        let target = offset + line;
        if target >= len {
            return None;
        }
        let already = *selected == target;
        *selected = target;
        Some(already)
    }

    /// The selection, first visible line and length of the list on screen.
    fn current_list(&mut self) -> (&mut usize, usize, usize) {
        let Screen::Tree(view) = &mut self.screen else {
            let len = self.disks.len();
            return (&mut self.disk_selected, self.disk_offset, len);
        };
        match view.panel.as_deref_mut() {
            None => (&mut view.selected, view.offset, view.rows.len()),
            Some(Panel::List(l)) => (&mut l.nav.selected, l.nav.offset, l.items.len()),
            Some(Panel::Errors(nav)) => {
                let len = view.progress.failed.lock().unwrap().len();
                (&mut nav.selected, nav.offset, len)
            }
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
        if view.panel.is_some() {
            self.on_key_panel(code);
            return;
        }
        match code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc | KeyCode::Char('b') => {
                self.stop_scan();
                self.disks = list_disks();
                self.screen = Screen::Disks;
            }
            KeyCode::Char('r') => self.rescan(),
            KeyCode::Char('o') => {
                let path = tree::path_of(&view.root, &view.rows, view.selected);
                let is_dir = view.selected_node().is_dir;
                self.reveal(&path, is_dir);
            }
            KeyCode::Char(key @ ('f' | 'c' | 'e')) => view.switch_panel(key),
            KeyCode::Char('d') => {
                let path = tree::path_of(&view.root, &view.rows, view.selected);
                self.ask_removal(path);
            }
            code => view.on_key(code),
        }
    }

    fn on_key_panel(&mut self, code: KeyCode) {
        let Screen::Tree(view) = &mut self.screen else {
            return;
        };
        match code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc | KeyCode::Char('b' | 'h') | KeyCode::Left => view.panel = None,
            KeyCode::Char(key @ ('f' | 'c' | 'e')) => view.switch_panel(key),
            KeyCode::Char('r') => self.rescan(),
            // Both lists hold things to find in their folder: a file, or a
            // folder that cannot be opened.
            KeyCode::Char('o') => {
                if let Some(path) = view.panel_selection() {
                    self.reveal(&path, false);
                }
            }
            KeyCode::Char('d') if matches!(view.panel.as_deref(), Some(Panel::List(_))) => {
                if let Some(path) = view.panel_selection() {
                    self.ask_removal(path);
                }
            }
            code => view.panel_key(code),
        }
    }

    fn rescan(&mut self) {
        if let Screen::Tree(view) = &self.screen {
            let path = view.path.clone();
            self.start_scan(&path);
        }
    }

    /// `d`: asks before moving `path` to the trash, or says why it will not.
    fn ask_removal(&mut self, path: PathBuf) {
        let Screen::Tree(view) = &self.screen else {
            return;
        };
        let refusal = if view.is_scanning() {
            Some("wait for the scan to finish before deleting".to_owned())
        } else if path == view.path {
            Some(format!(
                "{}: the scanned folder itself, go up a level to delete it",
                path.display()
            ))
        } else if platform::is_protected(&path) {
            Some(format!(
                "{}: a system folder, spacejunk does not delete it",
                path.display()
            ))
        } else {
            None
        };
        if refusal.is_some() {
            self.message = refusal;
            return;
        }
        if let Some(chain) = tree::find(&view.root, &path) {
            let size = chain.last().map_or(0, |n| n.size());
            self.confirm = Some(Removal { path, size });
        }
    }

    fn remove(&mut self, removal: &Removal) {
        let path = &removal.path;
        if let Err(e) = (self.trash)(path) {
            self.message = Some(format!("cannot delete {}: {e}", path.display()));
            return;
        }
        let Screen::Tree(view) = &mut self.screen else {
            return;
        };
        if let Some(chain) = tree::find(&view.root, path)
            && let Some((node, ancestors)) = chain.split_last()
        {
            node.remove(ancestors);
        }
        view.refresh();
        if let Some(Panel::List(list)) = view.panel.as_deref_mut() {
            list.rebuild(&view.root);
        }
        self.message = None;
        self.notice = Some(format!(
            "Moved {} ({}) to the {}: empty it to free the space",
            path.display(),
            crate::ui::human(removal.size),
            platform::TRASH_NAME
        ));
    }

    fn reveal(&mut self, path: &Path, is_dir: bool) {
        self.message = platform::reveal(path, is_dir)
            .err()
            .map(|e| format!("cannot open the file manager: {e}"));
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
            panel: None,
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
        let done = job.is_finished();
        if done {
            self.job = None;
            self.finished = Some(self.started.elapsed());
        }
        self.refresh();
        if let Some(Panel::List(list)) = self.panel.as_deref_mut()
            && (done || list.built.elapsed() >= LIST_REFRESH)
        {
            list.rebuild(&self.root);
        }
    }

    /// `f` / `c` / `e`: opens that list, or goes back to the tree if it is
    /// the one already open.
    fn switch_panel(&mut self, key: char) {
        let open = match self.panel.as_deref() {
            Some(Panel::List(l)) if l.kind == ListKind::LargestFiles => 'f',
            Some(Panel::List(_)) => 'c',
            Some(Panel::Errors(_)) => 'e',
            None => ' ',
        };
        let list = |kind| Panel::List(ListView::new(kind, &self.root));
        self.panel = match key {
            _ if key == open => None,
            'f' => Some(Box::new(list(ListKind::LargestFiles))),
            'c' => Some(Box::new(list(ListKind::Junk))),
            _ => Some(Box::new(Panel::Errors(ListNav::new()))),
        };
    }

    fn panel_selection(&self) -> Option<PathBuf> {
        match self.panel.as_deref()? {
            Panel::List(l) => l.items.get(l.nav.selected).map(|i| i.path.clone()),
            Panel::Errors(nav) => {
                let failed = self.progress.failed.lock().unwrap();
                failed.get(nav.selected).map(|(p, _)| p.clone())
            }
        }
    }

    fn panel_key(&mut self, code: KeyCode) {
        match self.panel.as_deref_mut() {
            Some(Panel::List(l)) => l.nav.on_key(code, l.items.len()),
            Some(Panel::Errors(nav)) => {
                nav.on_key(code, self.progress.failed.lock().unwrap().len());
            }
            None => {}
        }
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

impl ListView {
    fn new(kind: ListKind, root: &Node) -> Self {
        ListView {
            kind,
            items: Self::build(kind, root),
            nav: ListNav::new(),
            built: Instant::now(),
        }
    }

    fn build(kind: ListKind, root: &Node) -> Vec<Listed> {
        match kind {
            ListKind::LargestFiles => tree::largest_files(root, LIST_LIMIT),
            ListKind::Junk => tree::junk_folders(root, LIST_LIMIT),
        }
    }

    /// Re-reads the sizes; the selection stays on its entry if still listed.
    fn rebuild(&mut self, root: &Node) {
        let nav = &mut self.nav;
        let keep = self.items.get(nav.selected).map(|i| i.path.clone());
        self.items = Self::build(self.kind, root);
        self.built = Instant::now();
        let found = keep.and_then(|k| self.items.iter().position(|i| i.path == k));
        nav.selected = found
            .unwrap_or(nav.selected)
            .min(self.items.len().saturating_sub(1));
    }
}

impl ListNav {
    fn new() -> Self {
        ListNav {
            selected: 0,
            offset: 0,
            page: 10,
        }
    }

    fn on_key(&mut self, code: KeyCode, len: usize) {
        let last = len.saturating_sub(1);
        let page = self.page.max(1);
        self.selected = match code {
            KeyCode::Up | KeyCode::Char('k') => self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected + 1,
            KeyCode::PageUp => self.selected.saturating_sub(page),
            KeyCode::PageDown => self.selected + page,
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => last,
            _ => self.selected,
        }
        .min(last);
    }
}

#[cfg(test)]
impl App {
    /// An app showing `screen`, without listing the real disks or asking
    /// crates.io for updates.
    pub fn for_test(screen: Screen, disks: Vec<DiskEntry>) -> Self {
        App {
            screen,
            disks,
            disk_selected: 0,
            input: None,
            message: None,
            notice: None,
            confirm: None,
            update: None,
            quit: false,
            list_area: Rect::default(),
            disk_offset: 0,
            pool: Arc::new(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .unwrap(),
            ),
            update_rx: std::sync::mpsc::channel().1,
            trash: platform::trash,
        }
    }
}

#[cfg(test)]
impl TreeView {
    /// The view of a scan of `root` that already finished, in `secs`.
    pub fn finished(root: Node, files: u64, dirs: u64, secs: f64) -> Self {
        let progress = Progress::default();
        progress.files.store(files, Relaxed);
        progress.dirs.store(dirs, Relaxed);
        root.expanded.store(true, Relaxed);
        let mut view = TreeView {
            path: PathBuf::from(&*root.name),
            root: Arc::new(root),
            rows: Vec::new(),
            selected: 0,
            offset: 0,
            page: 10,
            progress: Arc::new(progress),
            panel: None,
            started: Instant::now(),
            finished: Some(Duration::from_secs_f64(secs)),
            job: None,
        };
        view.refresh();
        view
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
        let home = platform::home_dir().unwrap().to_string_lossy().into_owned();
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

    fn files_root(small: u64) -> Node {
        Node::fake_dir(
            "/r",
            vec![
                Node::fake_dir("a", vec![Node::fake_file("big", 50)]),
                Node::fake_file("small", small),
            ],
        )
    }

    fn file_names(files: &ListView) -> Vec<String> {
        let names = files.items.iter().map(|i| i.path.file_name().unwrap());
        names.map(|n| n.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn f_lists_the_largest_files_and_esc_goes_back_to_the_tree() {
        let view = TreeView::finished(files_root(1), 2, 2, 0.1);
        let mut app = App::for_test(Screen::Tree(view), Vec::new());
        app.on_key(KeyCode::Char('f').into());
        app.on_key(KeyCode::Down.into());
        let Screen::Tree(view) = &app.screen else {
            unreachable!()
        };
        let Some(Panel::List(files)) = view.panel.as_deref() else {
            panic!("files view not open")
        };
        assert_eq!(file_names(files), ["big", "small"]);
        assert_eq!(files.nav.selected, 1);
        app.on_key(KeyCode::Esc.into());
        let Screen::Tree(view) = &app.screen else {
            panic!("Esc left the scan instead of the files view")
        };
        assert!(view.panel.is_none());
    }

    #[test]
    fn e_lists_unreadable_folders_and_f_switches_lists() {
        let view = TreeView::finished(files_root(1), 2, 2, 0.1);
        view.progress
            .failed
            .lock()
            .unwrap()
            .push(("/r/locked".into(), "Permission denied".into()));
        let mut app = App::for_test(Screen::Tree(view), Vec::new());
        let panel = |app: &App| match &app.screen {
            Screen::Tree(v) => match v.panel.as_deref() {
                None => "tree",
                Some(Panel::List(_)) => "files",
                Some(Panel::Errors(_)) => "errors",
            },
            Screen::Disks => "disks",
        };
        app.on_key(KeyCode::Char('e').into());
        assert_eq!(panel(&app), "errors");
        let Screen::Tree(view) = &app.screen else {
            unreachable!()
        };
        assert_eq!(view.panel_selection(), Some(PathBuf::from("/r/locked")));
        app.on_key(KeyCode::Char('f').into());
        assert_eq!(panel(&app), "files");
        app.on_key(KeyCode::Char('f').into());
        assert_eq!(panel(&app), "tree");
    }

    fn mouse(kind: MouseEventKind, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: 10,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn click_selects_a_line_and_a_second_click_opens_it() {
        let view = TreeView::finished(files_root(1), 2, 2, 0.1); // /r, a/, small
        let mut app = App::for_test(Screen::Tree(view), Vec::new());
        app.list_area = Rect::new(1, 3, 78, 6);
        let click = MouseEventKind::Down(MouseButton::Left);
        app.on_mouse(mouse(click, 4));
        let Screen::Tree(view) = &app.screen else {
            unreachable!()
        };
        assert_eq!(selected_name(view), "a");
        assert_eq!(view.rows.len(), 3);
        app.on_mouse(mouse(click, 4));
        app.on_mouse(mouse(click, 1)); // outside the list: ignored
        app.on_mouse(mouse(click, 8)); // below the last row: ignored
        let Screen::Tree(view) = &app.screen else {
            unreachable!()
        };
        assert_eq!(view.rows.len(), 4, "a opened");
        assert_eq!(selected_name(view), "a");
    }

    #[test]
    fn the_selection_follows_the_pointer_and_a_click_then_opens() {
        let view = TreeView::finished(files_root(1), 2, 2, 0.1); // /r, a/, small
        let mut app = App::for_test(Screen::Tree(view), Vec::new());
        app.list_area = Rect::new(1, 3, 78, 6);
        app.on_mouse(mouse(MouseEventKind::Moved, 5));
        assert_eq!(selected_name(tree(&app)), "small");
        app.on_mouse(mouse(MouseEventKind::Moved, 8)); // below the last row
        app.on_mouse(mouse(MouseEventKind::Moved, 1)); // outside the list
        assert_eq!(selected_name(tree(&app)), "small", "kept");
        app.on_mouse(mouse(MouseEventKind::Moved, 4));
        assert_eq!(tree(&app).rows.len(), 3, "pointing does not open");
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4));
        assert_eq!(tree(&app).rows.len(), 4, "one click opens a/");
    }

    #[test]
    fn wheel_moves_the_selection_three_lines() {
        let view = TreeView::finished(files_root(1), 2, 2, 0.1);
        let mut app = App::for_test(Screen::Tree(view), Vec::new());
        app.on_key(KeyCode::Char('f').into());
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 0));
        let Screen::Tree(view) = &app.screen else {
            unreachable!()
        };
        let Some(Panel::List(files)) = view.panel.as_deref() else {
            panic!("files view not open")
        };
        assert_eq!(files.nav.selected, 1, "clamped to the last of 2 files");
    }

    fn trash_app() -> App {
        let view = TreeView::finished(files_root(1), 2, 2, 0.1); // /r, a/, small
        let mut app = App::for_test(Screen::Tree(view), Vec::new());
        app.trash = |path| {
            TRASHED.with(|t| t.borrow_mut().push(path.to_path_buf()));
            Ok(())
        };
        app
    }

    thread_local! {
        static TRASHED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn tree(app: &App) -> &TreeView {
        let Screen::Tree(view) = &app.screen else {
            panic!("not on the tree")
        };
        view
    }

    #[test]
    fn d_asks_first_and_y_moves_to_the_trash() {
        let mut app = trash_app();
        app.on_key(KeyCode::Down.into()); // a/
        app.on_key(KeyCode::Char('d').into());
        let a = PathBuf::from("/r").join("a");
        assert_eq!(app.confirm.as_ref().map(|c| &c.path), Some(&a));
        app.on_key(KeyCode::Char('n').into());
        assert!(app.confirm.is_none(), "any other key cancels");
        assert!(TRASHED.with(|t| t.borrow().is_empty()));

        app.on_key(KeyCode::Char('d').into());
        app.on_key(KeyCode::Char('y').into());
        assert_eq!(TRASHED.with(|t| t.borrow().clone()), [a]);
        let view = tree(&app);
        assert_eq!(view.root.size(), 1, "a's 50 bytes left the total");
        assert_eq!(view.rows.len(), 2, "/r and small");
        assert!(app.notice.as_deref().unwrap().contains("(50 B)"));
    }

    #[test]
    fn d_refuses_the_scanned_folder_itself() {
        let mut app = trash_app();
        app.on_key(KeyCode::Char('d').into());
        assert!(app.confirm.is_none());
        assert!(app.message.is_some());
    }

    #[test]
    fn d_in_the_largest_files_list_trashes_that_file() {
        let mut app = trash_app();
        app.on_key(KeyCode::Char('f').into());
        app.on_key(KeyCode::Char('d').into());
        app.on_key(KeyCode::Char('y').into());
        let big = PathBuf::from("/r").join("a").join("big");
        assert_eq!(TRASHED.with(|t| t.borrow().clone()), [big]);
        let Some(Panel::List(files)) = tree(&app).panel.as_deref() else {
            panic!("files view closed")
        };
        assert_eq!(file_names(files), ["small"]);
    }

    #[cfg(unix)]
    #[test]
    fn d_refuses_system_folders() {
        let root = Node::fake_dir(
            "/",
            vec![Node::fake_dir("usr", vec![Node::fake_file("x", 9)])],
        );
        let view = TreeView::finished(root, 1, 2, 0.1);
        let mut app = App::for_test(Screen::Tree(view), Vec::new());
        app.on_key(KeyCode::Down.into());
        app.on_key(KeyCode::Char('d').into());
        assert!(app.confirm.is_none());
        assert!(app.message.as_deref().unwrap().contains("system"));
    }

    #[test]
    fn c_lists_the_junk_and_d_can_trash_it() {
        let root = Node::fake_dir(
            "/r",
            vec![Node::fake_dir(
                "app",
                vec![Node::fake_dir("__pycache__", vec![Node::fake_file("x", 7)])],
            )],
        );
        let mut app = App::for_test(
            Screen::Tree(TreeView::finished(root, 1, 3, 0.1)),
            Vec::new(),
        );
        app.trash = |_| Ok(());
        app.on_key(KeyCode::Char('c').into());
        let Some(Panel::List(junk)) = tree(&app).panel.as_deref() else {
            panic!("junk list not open")
        };
        assert_eq!(junk.kind, ListKind::Junk);
        assert_eq!(file_names(junk), ["__pycache__"]);
        app.on_key(KeyCode::Char('d').into());
        app.on_key(KeyCode::Char('y').into());
        assert_eq!(tree(&app).root.size(), 0);
        app.on_key(KeyCode::Char('c').into());
        assert!(tree(&app).panel.is_none(), "c again closes it");
    }

    #[test]
    fn files_selection_follows_its_file_when_the_list_reorders() {
        let mut files = ListView::new(ListKind::LargestFiles, &files_root(1));
        files.nav.selected = 1; // small
        files.rebuild(&files_root(100)); // small is now the largest
        assert_eq!(file_names(&files), ["small", "big"]);
        assert_eq!(files.nav.selected, 0);
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

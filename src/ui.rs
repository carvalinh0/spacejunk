//! Rendering.

use std::sync::atomic::Ordering::Relaxed;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::app::{App, Screen, TreeView};
use crate::scan::Node;
use crate::tree::{self, NONE};
use crate::update;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const DIR_STYLE: Style = Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD);
/// Background only: reversing the colors (the old style) swapped the bar's
/// filled and empty cells on the selected line, so the bar seemed to jump.
const SELECTED: Style = Style::new()
    .bg(Color::DarkGray)
    .add_modifier(Modifier::BOLD);

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

/// Horizontal bar with 1/8 cell resolution.
fn bar(frac: f64, width: usize) -> String {
    const PARTS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = (frac.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
    let mut s = String::with_capacity(width * 3);
    for _ in 0..eighths / 8 {
        s.push('█');
    }
    if eighths / 8 < width {
        s.push(PARTS[eighths % 8]);
        for _ in eighths / 8 + 1..width {
            s.push(' ');
        }
    }
    s
}

fn heat(frac: f64) -> Color {
    if frac >= 0.5 {
        Color::Red
    } else if frac >= 0.2 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn spinner() -> char {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    SPINNER[(ms / 100) as usize % SPINNER.len()]
}

fn footer(keys: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (k, d) in keys {
        spans.push(Span::styled(format!(" {k} "), Style::new().reversed()));
        spans.push(Span::raw(format!(" {d}  ")));
    }
    Line::from(spans)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [title, body, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    f.render_widget(title_line(app), title);

    match &mut app.screen {
        Screen::Disks => draw_disks(f, body, app.disk_selected, &app.disks),
        Screen::Tree(view) => draw_tree(f, body, view),
    }
    f.render_widget(footer_for(app), foot);
}

fn title_line(app: &App) -> Line<'static> {
    let mut spans = vec![" spacejunk ".bold().black().on_cyan(), Span::raw(" ")];
    if let Some(latest) = &app.update {
        spans.push(update_notice(latest).green().bold());
        spans.push(Span::raw("  "));
    }
    spans.push(Span::raw(app.message.clone().unwrap_or_default()).red());
    Line::from(spans)
}

fn update_notice(latest: &str) -> Span<'static> {
    Span::raw(format!(
        "⬆ {latest} available (you have {}): rerun the installer or `cargo install spacejunk`",
        update::CURRENT
    ))
}

fn footer_for(app: &App) -> Line<'static> {
    if let Some(text) = &app.input {
        return Line::from(vec![
            Span::styled(" Path ", Style::new().reversed()),
            Span::raw(format!(" {text}")),
            Span::styled("█", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            Span::raw("   Enter scan · Esc cancel").dim(),
        ]);
    }
    match &app.screen {
        Screen::Disks => footer(&[
            ("↑↓", "move"),
            ("Enter", "scan"),
            ("p", "type a path"),
            ("r", "refresh"),
            ("q", "quit"),
        ]),
        Screen::Tree(_) => footer(&[
            ("↑↓", "move"),
            ("→/←", "open/close"),
            ("Enter", "toggle"),
            ("r", "rescan"),
            ("Esc", "disks"),
            ("q", "quit"),
        ]),
    }
}

fn draw_disks(f: &mut Frame, area: Rect, selected: usize, disks: &[crate::app::DiskEntry]) {
    let block = Block::bordered().title(" Select a disk ");
    if disks.is_empty() {
        let text = "No disks found. Press p to type a path.";
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }
    let rows = disks.iter().map(|d| {
        let frac = d.used as f64 / d.total as f64;
        Row::new(vec![
            Cell::from(d.mount.to_string_lossy().into_owned()).bold(),
            Cell::from(d.device.clone()),
            Cell::from(d.fs.clone()),
            Cell::from(human(d.used)),
            Cell::from(human(d.total)),
            Cell::from(Line::from(vec![
                Span::styled(bar(frac, 20), Style::new().fg(heat(frac))),
                Span::raw(format!(" {:>3.0}%", frac * 100.0)),
            ])),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Fill(2),
            Constraint::Fill(2),
            Constraint::Length(8),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(26),
        ],
    )
    .header(Row::new(["Mount", "Device", "FS", "Used", "Total", "Usage"]).underlined())
    .row_highlight_style(SELECTED)
    .highlight_symbol("▶ ")
    .block(block);
    let mut state = TableState::default().with_selected(Some(selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn summary(view: &TreeView) -> Line<'static> {
    let p = &view.progress;
    let files = p.files.load(Relaxed);
    let secs = view.elapsed().as_secs_f64();
    let counts = format!(
        "{}  ·  {files} files  ·  {} dirs  ·  {} errors",
        human(view.root.size()),
        p.dirs.load(Relaxed),
        p.errors.load(Relaxed),
    );
    if view.is_scanning() {
        let rate = files as f64 / secs.max(0.001);
        Line::from(format!(
            " {} scanning {}  {counts}  ·  {secs:.1}s  ·  {rate:.0} files/s",
            spinner(),
            view.path.display(),
        ))
        .yellow()
        .bold()
    } else {
        Line::from(format!(" {}  {counts}  ·  {secs:.2}s", view.path.display())).bold()
    }
}

fn draw_tree(f: &mut Frame, area: Rect, view: &mut TreeView) {
    let [info, list, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    f.render_widget(summary(view), info);

    // Only build rows for the lines on screen: the row list can be huge.
    let height = list.height.saturating_sub(2) as usize; // borders
    view.page = height;
    if view.selected < view.offset {
        view.offset = view.selected;
    } else if height > 0 && view.selected >= view.offset + height {
        view.offset = view.selected + 1 - height;
    }
    let end = (view.offset + height).min(view.rows.len());
    let rows = (view.offset..end).map(|i| tree_row(view, i));
    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Length(6),
            Constraint::Length(12),
            Constraint::Fill(1),
        ],
    )
    .row_highlight_style(SELECTED)
    .block(Block::bordered());
    let mut state = TableState::default().with_selected(Some(view.selected - view.offset));
    f.render_stateful_widget(table, list, &mut state);

    let path = tree::path_of(&view.root, &view.rows, view.selected);
    f.render_widget(Line::from(format!(" {}", path.display())).dim(), status);
}

/// Share of the parent's size (the root is 100%).
fn share(size: u64, parent_size: u64) -> f64 {
    if parent_size == 0 {
        0.0
    } else {
        (size as f64 / parent_size as f64).min(1.0)
    }
}

fn node_label(n: &Node, depth: u16) -> Line<'static> {
    let marker = if n.is_pending() {
        Span::raw(format!("{} ", spinner())).yellow()
    } else if !n.can_expand() {
        Span::raw("  ")
    } else if n.expanded.load(Relaxed) {
        Span::raw("▾ ")
    } else {
        Span::raw("▸ ")
    };
    let indent = Span::raw("  ".repeat(depth as usize));
    // The root is named after its full path, which may already end in a separator.
    let name = if !n.is_dir {
        Span::raw(n.name.to_string())
    } else if n.name.ends_with(['/', '\\']) {
        Span::styled(n.name.to_string(), DIR_STYLE)
    } else {
        Span::styled(format!("{}/", n.name), DIR_STYLE)
    };
    Line::from(vec![indent, marker, name])
}

fn tree_row(view: &TreeView, i: usize) -> Row<'static> {
    let row = view.rows[i];
    let n = tree::node(&view.root, &view.rows, i);
    let size = n.size();
    let parent_size = if row.parent == NONE {
        size
    } else {
        tree::node(&view.root, &view.rows, row.parent as usize).size()
    };
    let frac = share(size, parent_size);
    Row::new(vec![
        Cell::from(format!("{:>10}", human(size))).fg(heat(frac)),
        Cell::from(format!("{:>5.1}%", frac * 100.0)),
        Cell::from(bar(frac, 12)).fg(heat(frac)),
        Cell::from(node_label(n, row.depth)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::DiskEntry;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::path::PathBuf;

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    fn render(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        t.backend().buffer().clone()
    }

    fn lines(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (0..area.height)
            .map(|y| {
                let line: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
                line.trim_end().to_owned()
            })
            .collect()
    }

    /// `/data` (open) > `videos/` (open) > `movie.mkv`, and the rest closed.
    fn tree_app() -> App {
        let videos = Node::fake_dir(
            "videos",
            vec![
                Node::fake_file("movie.mkv", 6 * GIB),
                Node::fake_file("clip.mp4", 512 * MIB),
            ],
        );
        let root = Node::fake_dir(
            "/data",
            vec![
                Node::fake_file("notes.txt", 4096),
                videos,
                Node::fake_dir("photos", vec![Node::fake_file("a.jpg", 2 * GIB)]),
            ],
        );
        root.children()[1].expanded.store(true, Relaxed);
        App::for_test(
            Screen::Tree(TreeView::finished(root, 4, 3, 0.25)),
            Vec::new(),
        )
    }

    #[test]
    fn update_notice_names_both_versions() {
        let text = update_notice("9.9.9").content.into_owned();
        assert!(
            text.contains("9.9.9") && text.contains(update::CURRENT),
            "{text}"
        );
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1024), "1.0 KiB");
        assert_eq!(human(1536 * 1024 * 1024), "1.5 GiB");
    }

    #[test]
    fn bar_width_is_constant() {
        for frac in [0.0, 0.01, 0.5, 0.99, 1.0] {
            assert_eq!(bar(frac, 12).chars().count(), 12);
        }
    }

    #[test]
    fn share_is_capped_while_sizes_race() {
        // A child can be read after it grew but before its parent did.
        assert!((share(150, 100) - 1.0).abs() < f64::EPSILON);
        assert!(share(5, 0).abs() < f64::EPSILON);
    }

    fn expected(screen: &str) -> Vec<String> {
        screen
            .strip_prefix('\n')
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn tree_screen() {
        let buf = render(&mut tree_app(), 80, 12);
        let screen = r"
 spacejunk
 /data  8.5 GiB  ·  4 files  ·  3 dirs  ·  0 errors  ·  0.25s
┌──────────────────────────────────────────────────────────────────────────────┐
│   8.5 GiB  100.0% ████████████ ▾ /data/                                      │
│   6.5 GiB   76.5% █████████▏     ▾ videos/                                   │
│   6.0 GiB   92.3% ███████████▏       movie.mkv                               │
│ 512.0 MiB    7.7% ▉                  clip.mp4                                │
│   2.0 GiB   23.5% ██▉            ▸ photos/                                   │
│   4.0 KiB    0.0%                  notes.txt                                 │
└──────────────────────────────────────────────────────────────────────────────┘
 /data
 ↑↓  move   →/←  open/close   Enter  toggle   r  rescan   Esc  disks   q  quit";
        assert_eq!(lines(&buf), expected(screen));
    }

    /// The bug `SELECTED` fixed: on the selected line the bar must keep its
    /// cells and colors, only the background changes.
    #[test]
    fn selecting_a_line_only_changes_its_background() {
        let mut app = tree_app();
        let before = render(&mut app, 80, 12);
        let Screen::Tree(view) = &mut app.screen else {
            unreachable!()
        };
        view.selected = 1; // videos/
        let after = render(&mut app, 80, 12);
        // What the terminal shows: REVERSED swaps the two colors on screen.
        let shown = |c: &ratatui::buffer::Cell| {
            if c.modifier.contains(Modifier::REVERSED) {
                (c.bg, c.fg)
            } else {
                (c.fg, c.bg)
            }
        };
        let y = 4;
        for x in 0..80 {
            let (old, new) = (&before[(x, y)], &after[(x, y)]);
            assert_eq!(old.symbol(), new.symbol(), "x={x}");
            assert_eq!(shown(old).0, shown(new).0, "x={x}: text color changed");
            if (1..79).contains(&x) {
                assert_eq!(shown(new).1, Color::DarkGray, "x={x}");
            }
        }
    }

    #[test]
    fn disks_screen() {
        let disks = vec![
            DiskEntry {
                mount: PathBuf::from("/"),
                device: "/dev/sda1".into(),
                fs: "ext4".into(),
                total: 100 * GIB,
                used: 25 * GIB,
            },
            DiskEntry {
                mount: PathBuf::from("/mnt/backup"),
                device: "/dev/sdb1".into(),
                fs: "btrfs".into(),
                total: 2048 * GIB,
                used: 1843 * GIB,
            },
        ];
        let buf = render(&mut App::for_test(Screen::Disks, disks), 100, 7);
        let screen = r"
 spacejunk
┌ Select a disk ───────────────────────────────────────────────────────────────────────────────────┐
│  Mount              Device            FS       Used        Total       Usage                     │
│▶ /                  /dev/sda1         ext4     25.0 GiB    100.0 GiB   █████                 25% │
│  /mnt/backup        /dev/sdb1         btrfs    1.8 TiB     2.0 TiB     ██████████████████    90% │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
 ↑↓  move   Enter  scan   p  type a path   r  refresh   q  quit";
        assert_eq!(lines(&buf), expected(screen));
    }

    #[test]
    fn selected_line_keeps_bar_colors() {
        assert!(!SELECTED.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(SELECTED.fg, None, "the bar keeps its own color");
    }
}

//! Rendering.

use std::sync::atomic::Ordering::Relaxed;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use std::path::{Path, PathBuf};

use crate::app::{App, ListKind, ListNav, ListView, Panel, Screen, TreeView};
use crate::junk;
use crate::platform;
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

/// Key hints in order of importance. Hints that do not fit the width are
/// dropped, except the last one (quit), which always shows.
fn footer(keys: &[(&str, &str)], width: u16) -> Line<'static> {
    let hint_width = |(k, d): &(&str, &str)| k.chars().count() + d.chars().count() + 5;
    let Some((last, rest)) = keys.split_last() else {
        return Line::default();
    };
    let mut room = usize::from(width).saturating_sub(hint_width(last));
    let mut shown: Vec<&(&str, &str)> = rest
        .iter()
        .take_while(|hint| {
            let fits = hint_width(hint) <= room;
            room = room.saturating_sub(hint_width(hint));
            fits
        })
        .collect();
    shown.push(last);
    let mut spans = Vec::new();
    for (k, d) in shown {
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

    app.list_area = match &mut app.screen {
        Screen::Disks => {
            let (lines, offset) = draw_disks(f, body, app.disk_selected, &app.disks);
            app.disk_offset = offset;
            lines
        }
        Screen::Tree(view) => draw_tree(f, body, view),
    };
    f.render_widget(footer_for(app, foot.width), foot);
}

fn title_line(app: &App) -> Line<'static> {
    let mut spans = vec![" spacejunk ".bold().black().on_cyan(), Span::raw(" ")];
    if let Some(latest) = &app.update {
        spans.push(update_notice(latest).green().bold());
        spans.push(Span::raw("  "));
    }
    spans.push(Span::raw(app.message.clone().unwrap_or_default()).red());
    spans.push(Span::raw(app.notice.clone().unwrap_or_default()).green());
    Line::from(spans)
}

fn update_notice(latest: &str) -> Span<'static> {
    Span::raw(format!(
        "⬆ {latest} available (you have {}): rerun the installer or `cargo install spacejunk`",
        update::CURRENT
    ))
}

fn footer_for(app: &App, width: u16) -> Line<'static> {
    if let Some(text) = &app.input {
        return Line::from(vec![
            Span::styled(" Path ", Style::new().reversed()),
            Span::raw(format!(" {text}")),
            Span::styled("█", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            Span::raw("   Enter scan · Esc cancel").dim(),
        ]);
    }
    if let Some(removal) = &app.confirm {
        return Line::from(vec![
            Span::styled(" Delete ", Style::new().reversed().red()),
            Span::raw(format!(
                " Move {} ({}) to the {}?",
                removal.path.display(),
                human(removal.size),
                platform::TRASH_NAME
            )),
            Span::raw("   y yes · any other key no").dim(),
        ]);
    }
    let keys: &[(&str, &str)] = match &app.screen {
        Screen::Disks => &[
            ("↑↓", "move"),
            ("Enter", "scan"),
            ("p", "type a path"),
            ("r", "refresh"),
            ("q", "quit"),
        ],
        Screen::Tree(view) => match view.panel.as_deref() {
            Some(Panel::List(list)) if list.kind == ListKind::LargestFiles => &[
                ("↑↓", "move"),
                ("o", "file manager"),
                ("d", "delete"),
                ("f/Esc", "back to the tree"),
                ("r", "rescan"),
                ("q", "quit"),
            ],
            Some(Panel::List(_)) => &[
                ("↑↓", "move"),
                ("o", "file manager"),
                ("d", "delete"),
                ("c/Esc", "back to the tree"),
                ("r", "rescan"),
                ("q", "quit"),
            ],
            Some(Panel::Errors(_)) => &[
                ("↑↓", "move"),
                ("o", "file manager"),
                ("e/Esc", "back to the tree"),
                ("r", "rescan"),
                ("q", "quit"),
            ],
            None => &[
                ("↑↓", "move"),
                ("→/←", "open/close"),
                ("f", "largest files"),
                ("c", "junk"),
                ("o", "file manager"),
                ("d", "delete"),
                ("e", "errors"),
                ("r", "rescan"),
                ("Esc", "disks"),
                ("q", "quit"),
            ],
        },
    };
    footer(keys, width)
}

/// Where the lines of a bordered list with `header` lines go, for mouse clicks.
fn lines_area(list: Rect, header: u16) -> Rect {
    let inner = list.inner(ratatui::layout::Margin::new(1, 1));
    Rect {
        y: inner.y + header,
        height: inner.height.saturating_sub(header),
        ..inner
    }
}

/// Returns where the disk lines are and the first disk shown.
fn draw_disks(
    f: &mut Frame,
    area: Rect,
    selected: usize,
    disks: &[crate::app::DiskEntry],
) -> (Rect, usize) {
    let block = Block::bordered().title(" Select a disk ");
    if disks.is_empty() {
        let text = "No disks found. Press p to type a path.";
        f.render_widget(Paragraph::new(text).block(block), area);
        return (Rect::default(), 0);
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
    (lines_area(area, 1), state.offset())
}

fn summary(view: &TreeView) -> Line<'static> {
    let p = &view.progress;
    let files = p.files.load(Relaxed);
    let secs = view.elapsed().as_secs_f64();
    let errors = match p.errors.load(Relaxed) {
        0 => "0 errors".to_owned(),
        n => format!("{n} errors (e: list)"),
    };
    let counts = format!(
        "{}  ·  {files} files  ·  {} dirs  ·  {errors}",
        human(view.root.size()),
        p.dirs.load(Relaxed),
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

/// Returns where the lines of the list on screen are.
fn draw_tree(f: &mut Frame, area: Rect, view: &mut TreeView) -> Rect {
    let [info, list, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    f.render_widget(summary(view), info);
    let total = view.root.size();
    match view.panel.as_deref_mut() {
        Some(Panel::List(items)) => {
            draw_list(f, [list, status], items, &view.path, total);
            return lines_area(list, 0);
        }
        Some(Panel::Errors(nav)) => {
            let failed = view.progress.failed.lock().unwrap();
            draw_errors(f, [list, status], nav, &failed, &view.path);
            return lines_area(list, 0);
        }
        None => {}
    }

    // Only build rows for the lines on screen: the row list can be huge.
    let height = list.height.saturating_sub(2) as usize; // borders
    view.page = height;
    scroll(&mut view.offset, view.selected, height);
    let end = (view.offset + height).min(view.rows.len());
    let rows = (view.offset..end).map(|i| tree_row(view, i));
    let table = Table::new(rows, columns())
        .row_highlight_style(SELECTED)
        .block(Block::bordered());
    let mut state = TableState::default().with_selected(Some(view.selected - view.offset));
    f.render_stateful_widget(table, list, &mut state);

    let path = tree::path_of(&view.root, &view.rows, view.selected);
    f.render_widget(Line::from(format!(" {}", path.display())).dim(), status);
    lines_area(list, 0)
}

/// Keeps the selected line inside the `height` visible lines.
fn scroll(offset: &mut usize, selected: usize, height: usize) {
    if selected < *offset {
        *offset = selected;
    } else if height > 0 && selected >= *offset + height {
        *offset = selected + 1 - height;
    }
}

/// Fits the list's selection and scroll to `list`; returns the visible range.
fn visible(nav: &mut ListNav, list: Rect, len: usize) -> std::ops::Range<usize> {
    let height = list.height.saturating_sub(2) as usize; // borders
    nav.page = height;
    nav.selected = nav.selected.min(len.saturating_sub(1));
    scroll(&mut nav.offset, nav.selected, height);
    nav.offset..(nav.offset + height).min(len)
}

fn draw_list(
    f: &mut Frame,
    [list, status]: [Rect; 2],
    view: &mut ListView,
    base: &Path,
    total: u64,
) {
    let items = &view.items;
    let block = match view.kind {
        ListKind::LargestFiles => {
            Block::bordered().title(format!(" Largest files ({}) ", items.len()))
        }
        ListKind::Junk => {
            let junk: u64 = items.iter().map(|i| i.size).sum();
            Block::bordered()
                .title(format!(" Junk: {}, {} ", folders(items.len()), human(junk)))
                .title_bottom(" Rebuilt by the programs that made them when needed ")
        }
    };
    if items.is_empty() {
        let text = match view.kind {
            ListKind::LargestFiles => " No files yet.",
            ListKind::Junk => " No junk found: no caches, build folders or temporary files.",
        };
        f.render_widget(Paragraph::new(text).block(block), list);
        return;
    }
    let shown = visible(&mut view.nav, list, items.len());
    let rows = items[shown].iter().map(|item| {
        let frac = share(item.size, total);
        let path = item.path.strip_prefix(base).unwrap_or(&item.path);
        let mut name = vec![Span::raw(path.display().to_string())];
        if !item.tag.is_empty() {
            name.push(Span::raw(format!("  {}", item.tag)).dim());
        }
        Row::new(vec![
            Cell::from(format!("{:>10}", human(item.size))).fg(heat(frac)),
            Cell::from(format!("{:>5.1}%", frac * 100.0)),
            Cell::from(bar(frac, 12)).fg(heat(frac)),
            Cell::from(Line::from(name)),
        ])
    });
    let table = Table::new(rows, columns())
        .row_highlight_style(SELECTED)
        .block(block);
    let nav = &view.nav;
    let mut state = TableState::default().with_selected(Some(nav.selected - nav.offset));
    f.render_stateful_widget(table, list, &mut state);
    let path = &items[nav.selected].path;
    f.render_widget(Line::from(format!(" {}", path.display())).dim(), status);
}

fn draw_errors(
    f: &mut Frame,
    [list, status]: [Rect; 2],
    nav: &mut ListNav,
    failed: &[(PathBuf, String)],
    base: &Path,
) {
    let block = Block::bordered()
        .title(format!(" Could not read ({}) ", failed.len()))
        .title_bottom(format!(" {} ", platform::ELEVATE_HINT));
    if failed.is_empty() {
        f.render_widget(
            Paragraph::new(" Every folder could be read.").block(block),
            list,
        );
        return;
    }
    let shown = visible(nav, list, failed.len());
    let rows = failed[shown].iter().map(|(path, reason)| {
        let shown = path.strip_prefix(base).unwrap_or(path);
        Row::new(vec![
            Cell::from(shown.display().to_string()),
            Cell::from(reason.clone()).dim(),
        ])
    });
    // Reasons are short ("Permission denied (os error 13)"): give them their
    // width and the path the rest.
    let reason_width = failed.iter().map(|(_, r)| r.chars().count()).max();
    let reason_width = reason_width.unwrap_or(0).min(40) as u16;
    let table = Table::new(
        rows,
        [Constraint::Fill(1), Constraint::Length(reason_width)],
    )
    .row_highlight_style(SELECTED)
    .block(block);
    let mut state = TableState::default().with_selected(Some(nav.selected - nav.offset));
    f.render_stateful_widget(table, list, &mut state);
    let path = &failed[nav.selected].0;
    f.render_widget(Line::from(format!(" {}", path.display())).dim(), status);
}

fn folders(n: usize) -> String {
    if n == 1 {
        "1 folder".to_owned()
    } else {
        format!("{n} folders")
    }
}

/// Size, share, bar, name: the tree and the largest files list line up.
fn columns() -> [Constraint; 4] {
    [
        Constraint::Length(11),
        Constraint::Length(6),
        Constraint::Length(12),
        Constraint::Fill(1),
    ]
}

/// Share of the parent's size (the root is 100%).
fn share(size: u64, parent_size: u64) -> f64 {
    if parent_size == 0 {
        0.0
    } else {
        (size as f64 / parent_size as f64).min(1.0)
    }
}

fn node_label(n: &Node, depth: u16, junk: Option<&str>) -> Line<'static> {
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
    let junk = junk.map_or_else(Span::default, |what| {
        Span::raw(format!("  junk: {what}")).dim()
    });
    Line::from(vec![indent, marker, name, junk])
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
    // Only folders can be junk; their path costs one walk up the rows.
    let junk = (n.is_dir && row.parent != NONE)
        .then(|| {
            let parent = tree::node(&view.root, &view.rows, row.parent as usize);
            junk::kind(n, parent, &tree::path_of(&view.root, &view.rows, i))
        })
        .flatten();
    Row::new(vec![
        Cell::from(format!("{:>10}", human(size))).fg(heat(frac)),
        Cell::from(format!("{:>5.1}%", frac * 100.0)),
        Cell::from(bar(frac, 12)).fg(heat(frac)),
        Cell::from(node_label(n, row.depth, junk)),
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
 ↑↓  move   →/←  open/close   f  largest files   c  junk   q  quit";
        assert_eq!(lines(&buf), expected(screen));
    }

    #[test]
    fn files_screen() {
        let mut app = tree_app();
        app.on_key(ratatui::crossterm::event::KeyCode::Char('f').into());
        let buf = render(&mut app, 80, 10);
        let sep = std::path::MAIN_SEPARATOR;
        let screen = format!(
            r"
 spacejunk
 /data  8.5 GiB  ·  4 files  ·  3 dirs  ·  0 errors  ·  0.25s
┌ Largest files (4) ───────────────────────────────────────────────────────────┐
│   6.0 GiB   70.6% ████████▌    videos{sep}movie.mkv                              │
│   2.0 GiB   23.5% ██▉          photos{sep}a.jpg                                  │
│ 512.0 MiB    5.9% ▊            videos{sep}clip.mp4                               │
│   4.0 KiB    0.0%              notes.txt                                     │
└──────────────────────────────────────────────────────────────────────────────┘
 /data{sep}videos{sep}movie.mkv
 ↑↓  move   o  file manager   d  delete   f/Esc  back to the tree   q  quit"
        );
        assert_eq!(lines(&buf), expected(&screen));
    }

    /// `/code` (open) holds `node_modules/`, marked as junk in the tree.
    fn junk_app() -> App {
        let deps = Node::fake_dir("node_modules", vec![Node::fake_file("react.js", 3 * MIB)]);
        let code = Node::fake_dir(
            "/code",
            vec![
                deps,
                Node::fake_file("package.json", MIB / 2),
                Node::fake_file("package-lock.json", MIB / 2),
            ],
        );
        App::for_test(
            Screen::Tree(TreeView::finished(code, 2, 2, 0.1)),
            Vec::new(),
        )
    }

    #[test]
    fn junk_is_marked_in_the_tree_and_listed_by_c() {
        let mut app = junk_app();
        let buf = render(&mut app, 80, 8);
        assert_eq!(
            lines(&buf)[4],
            "│   3.0 MiB   75.0% █████████      ▸ node_modules/  junk: npm packages         │"
        );
        app.on_key(ratatui::crossterm::event::KeyCode::Char('c').into());
        let buf = render(&mut app, 80, 7);
        let screen = r"
 spacejunk
 /code  4.0 MiB  ·  2 files  ·  2 dirs  ·  0 errors  ·  0.10s
┌ Junk: 1 folder, 3.0 MiB ─────────────────────────────────────────────────────┐
│   3.0 MiB   75.0% █████████    node_modules  npm packages                    │
└ Rebuilt by the programs that made them when needed ──────────────────────────┘
 /code/node_modules
 ↑↓  move   o  file manager   d  delete   c/Esc  back to the tree   q  quit";
        let sep = std::path::MAIN_SEPARATOR.to_string();
        assert_eq!(
            lines(&buf),
            expected(&screen.replace("/code/", &format!("/code{sep}")))
        );
    }

    #[test]
    fn errors_screen() {
        let mut app = tree_app();
        let Screen::Tree(view) = &app.screen else {
            unreachable!()
        };
        let reason = "Permission denied (os error 13)";
        let locked = PathBuf::from("/data").join("photos").join("private");
        view.progress.errors.store(1, Relaxed);
        let failed = &view.progress.failed;
        failed.lock().unwrap().push((locked, reason.into()));
        app.on_key(ratatui::crossterm::event::KeyCode::Char('e').into());
        let buf = render(&mut app, 80, 7);
        let sep = std::path::MAIN_SEPARATOR;
        let hint = platform::ELEVATE_HINT;
        let bottom = format!("└ {hint} {}┘", "─".repeat(76 - hint.chars().count()));
        // Path column: what the reason (and the column gap) leaves of 78.
        let row = format!("│{:<47}{reason}│", format!("photos{sep}private"));
        let screen = format!(
            r"
 spacejunk
 /data  8.5 GiB  ·  4 files  ·  3 dirs  ·  1 errors (e: list)  ·  0.25s
┌ Could not read (1) ──────────────────────────────────────────────────────────┐
{row}
{bottom}
 /data{sep}photos{sep}private
 ↑↓  move   o  file manager   e/Esc  back to the tree   r  rescan   q  quit"
        );
        assert_eq!(lines(&buf), expected(&screen));
    }

    #[test]
    fn delete_asks_in_the_footer() {
        let mut app = tree_app();
        app.on_key(ratatui::crossterm::event::KeyCode::Down.into()); // videos/
        app.on_key(ratatui::crossterm::event::KeyCode::Char('d').into());
        let buf = render(&mut app, 100, 12);
        let path = PathBuf::from("/data").join("videos");
        let trash = platform::TRASH_NAME;
        let want = format!(
            " Delete  Move {} (6.5 GiB) to the {trash}?   y yes · any other key no",
            path.display()
        );
        assert_eq!(lines(&buf)[11], want);
    }

    #[test]
    fn footer_drops_hints_that_do_not_fit_but_keeps_quit() {
        let keys = [("a", "first"), ("b", "second"), ("q", "quit")];
        let text = |w| footer(&keys, w).to_string();
        assert_eq!(text(80), " a  first   b  second   q  quit  ");
        assert_eq!(text(28), " a  first   q  quit  ");
        assert_eq!(text(5), " q  quit  ");
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

    /// Mouse clicks map a screen line to a list line through `list_area`.
    #[test]
    fn list_area_is_where_the_lines_are_drawn() {
        let mut app = tree_app();
        let buf = render(&mut app, 80, 12);
        assert_eq!(app.list_area, Rect::new(1, 3, 78, 6));
        assert!(lines(&buf)[3].contains("/data/"), "first line at y=3");
        let disks = vec![DiskEntry {
            mount: PathBuf::from("/"),
            device: "/dev/sda1".into(),
            fs: "ext4".into(),
            total: GIB,
            used: 0,
        }];
        let mut app = App::for_test(Screen::Disks, disks);
        let buf = render(&mut app, 100, 7);
        assert_eq!(app.list_area, Rect::new(1, 3, 98, 2), "below the header");
        assert!(lines(&buf)[3].contains("/dev/sda1"));
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

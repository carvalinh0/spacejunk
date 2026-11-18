mod app;
mod junk;
mod platform;
mod scan;
mod tree;
mod ui;
mod update;

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;

use app::App;

const HELP: &str = "\
spacejunk - find what is eating your disk

USAGE:
    spacejunk            pick a disk in the TUI
    spacejunk <PATH>     scan PATH directly
    spacejunk --scan <PATH> [N] [--json]
                         no TUI: print the N largest entries (default 10),
                         as one line of JSON with --json

OPTIONS:
    -h, --help       show this help
    -V, --version    show version
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("spacejunk {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--scan") if args.len() > 1 => {
            let rest = &args[2..];
            let json = rest.iter().any(|a| a == "--json");
            let top = rest.iter().find(|a| *a != "--json");
            let top = top.and_then(|n| n.parse().ok()).unwrap_or(10);
            headless(Path::new(&args[1]), top, json)
        }
        Some(p) if p.starts_with('-') => {
            eprintln!("invalid usage: {}\n", args.join(" "));
            eprint!("{HELP}");
            ExitCode::FAILURE
        }
        start => tui(start.map(PathBuf::from)),
    }
}

fn tui(start: Option<PathBuf>) -> ExitCode {
    let mut terminal = ratatui::init();
    // ratatui's panic hook restores the screen but not mouse capture: left
    // on, every mouse move would print escape codes in the user's shell.
    let restore_screen = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        restore_screen(info);
    }));
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let res = run(&mut terminal, App::new(start));
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    if let Err(e) = res {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(terminal: &mut DefaultTerminal, mut app: App) -> io::Result<()> {
    while !app.quit {
        app.tick();
        terminal.draw(|f| ui::draw(f, &mut app))?;
        let timeout = if app.is_scanning() {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(1)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Scan without the TUI. Handy for scripts and benchmarks.
fn headless(path: &Path, top: usize, json: bool) -> ExitCode {
    let progress = scan::Progress::default();
    let start = Instant::now();
    let pool = app::build_pool();
    let root = match scan::Node::root(path) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("error: {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    pool.install(|| scan::scan(path, &root, &progress));
    let elapsed = start.elapsed();
    let entries = largest_children(&root, top);
    if json {
        let secs = elapsed.as_secs_f64();
        println!("{}", json_report(path, &root, &progress, secs, &entries));
        return ExitCode::SUCCESS;
    }
    println!(
        "{}  {}  ({} files, {} dirs, {} errors, {:.3}s)",
        ui::human(root.size()),
        path.display(),
        progress.files.load(Relaxed),
        progress.dirs.load(Relaxed),
        progress.errors.load(Relaxed),
        elapsed.as_secs_f64()
    );
    for c in entries {
        let slash = if c.is_dir { "/" } else { "" };
        println!("{:>10}  {}{slash}", ui::human(c.size()), c.name);
    }
    ExitCode::SUCCESS
}

fn largest_children(root: &scan::Node, top: usize) -> Vec<&scan::Node> {
    let mut children: Vec<&scan::Node> = root.children().iter().collect();
    children.sort_unstable_by_key(|c| std::cmp::Reverse(c.size()));
    children.truncate(top);
    children
}

/// The `--scan --json` answer: totals, then `entries` largest first. Sizes
/// are bytes, unformatted, so scripts can compare them.
fn json_report(
    path: &Path,
    root: &scan::Node,
    p: &scan::Progress,
    secs: f64,
    entries: &[&scan::Node],
) -> String {
    let entries: Vec<String> = entries
        .iter()
        .map(|c| {
            let name = json_string(&c.name);
            format!(
                r#"{{"name":{name},"size":{},"dir":{}}}"#,
                c.size(),
                c.is_dir
            )
        })
        .collect();
    format!(
        r#"{{"path":{},"size":{},"files":{},"dirs":{},"errors":{},"seconds":{secs:.3},"entries":[{}]}}"#,
        json_string(&path.to_string_lossy()),
        root.size(),
        p.files.load(Relaxed),
        p.dirs.load(Relaxed),
        p.errors.load(Relaxed),
        entries.join(",")
    )
}

/// No JSON crate for one line of output: quotes, backslashes and control
/// characters are all that file names need escaped.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_are_escaped() {
        let name = "a\"b\\c\nd\te\u{1}f/é";
        assert_eq!(json_string(name), r#""a\"b\\c\nd\te\u0001f/é""#);
    }

    #[test]
    fn json_report_has_totals_and_entries_largest_first() {
        let root = scan::Node::fake_dir(
            "/data",
            vec![
                scan::Node::fake_file("small", 10),
                scan::Node::fake_dir("big", vec![scan::Node::fake_file("x", 90)]),
            ],
        );
        let progress = scan::Progress::default();
        progress.files.store(2, Relaxed);
        progress.dirs.store(2, Relaxed);
        let entries = largest_children(&root, 10);
        assert_eq!(
            json_report(Path::new("/data"), &root, &progress, 0.5, &entries),
            r#"{"path":"/data","size":100,"files":2,"dirs":2,"errors":0,"seconds":0.500,"entries":[{"name":"big","size":90,"dir":true},{"name":"small","size":10,"dir":false}]}"#
        );
    }
}

mod app;
mod platform;
mod scan;
mod tree;
mod ui;
mod update;

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::App;

const HELP: &str = "\
spacejunk - find what is eating your disk

USAGE:
    spacejunk            pick a disk in the TUI
    spacejunk <PATH>     scan PATH directly
    spacejunk --scan <PATH> [N]
                         no TUI: print the N largest entries (default 10)

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
            let top = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(10);
            headless(Path::new(&args[1]), top)
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
    let res = run(&mut terminal, App::new(start));
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
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key);
        }
    }
    Ok(())
}

/// Scan without the TUI. Handy for scripts and benchmarks.
fn headless(path: &Path, top: usize) -> ExitCode {
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
    println!(
        "{}  {}  ({} files, {} dirs, {} errors, {:.3}s)",
        ui::human(root.size()),
        path.display(),
        progress.files.load(Relaxed),
        progress.dirs.load(Relaxed),
        progress.errors.load(Relaxed),
        elapsed.as_secs_f64()
    );
    let mut children: Vec<&scan::Node> = root.children().iter().collect();
    children.sort_unstable_by_key(|c| std::cmp::Reverse(c.size()));
    for c in children.into_iter().take(top) {
        let slash = if c.is_dir { "/" } else { "" };
        println!("{:>10}  {}{slash}", ui::human(c.size()), c.name);
    }
    ExitCode::SUCCESS
}

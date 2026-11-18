<p align="center"><img src="assets/logo-wordmark.svg" alt="spacejunk" width="560"></p>

# spacejunk

Find out what is eating your disk. A Rust TUI that scans a disk and shows a tree of folders
from **heaviest to lightest**, updated **live** while the scan runs.

<img width="1920" height="1080" alt="spacejunk-store-1" src="https://github.com/user-attachments/assets/1711ed24-ebe8-4d47-b1bf-a3bcefe69f1b" />

## Install

**Linux and macOS**

```sh
curl -fsSL https://raw.githubusercontent.com/carvalinh0/spacejunk/main/scripts/install.sh | sh
```

**Windows**: get it from the [Microsoft Store](https://apps.microsoft.com/detail/9NLB9QFQQRW3), or from a terminal:

```powershell
winget install 9NLB9QFQQRW3 --source msstore
```

The Store version is signed by Microsoft, so Windows 11 Smart App Control lets it run, and the Store
keeps it up to date. It adds the `spacejunk` command to every terminal and a Start Menu entry.

**Windows without the Store** (PowerShell)

```powershell
irm https://raw.githubusercontent.com/carvalinh0/spacejunk/main/scripts/install.ps1 | iex
```

The binary this script installs is not signed: Windows 11 blocks it when Smart App Control is on.

The scripts use `cargo install spacejunk` when Rust is installed. Without cargo, or if the build
fails, they download the latest release binary, verify its SHA-256 and install it to
`~/.local/bin` (Linux/macOS) or `%LOCALAPPDATA%\Programs\spacejunk` (Windows, added to your PATH).
Run the same command again to update.

They also add spacejunk to your app menu, so you can start it without typing a command: it opens
in a terminal window on the disk list.

| System | Where |
|---|---|
| Linux | application menu (`~/.local/share/applications/spacejunk.desktop`) |
| macOS | Launchpad / Spotlight (`~/Applications/spacejunk.app`) |
| Windows | Start Menu |

With Rust you can also run `cargo install spacejunk` directly (or `cargo install --path .` from a checkout);
that installs the command only, without the app menu entry.

### Uninstall

```sh
# Linux / macOS
rm -f ~/.local/bin/spacejunk                      # or: cargo uninstall spacejunk
rm -f ~/.local/share/applications/spacejunk.desktop ~/.local/share/icons/hicolor/scalable/apps/spacejunk.svg
rm -rf ~/Applications/spacejunk.app               # macOS
```

```powershell
# Windows
Remove-Item -Recurse "$env:LOCALAPPDATA\Programs\spacejunk"   # or: cargo uninstall spacejunk
Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\spacejunk.lnk"
```

On Windows, also remove `%LOCALAPPDATA%\Programs\spacejunk` from your user PATH (Settings > Environment variables).

Microsoft Store version: Settings > Apps > Installed apps > spacejunk > Uninstall, or
`winget uninstall 9NLB9QFQQRW3`.

### Update notice

On startup, spacejunk asks crates.io for the latest version in the background (through `curl`,
with a 5 s timeout). If there is a newer one, the title bar shows:

```
 spacejunk  ⬆ 0.2.0 available (you have 0.1.0): rerun the installer or `cargo install spacejunk`
```

Offline or without `curl`, nothing happens. The request only carries the program name, its version
and the repository URL (in the User-Agent, as crates.io asks). The Microsoft Store version skips the
check: the Store updates it.

## Usage

```sh
spacejunk              # pick a disk (or type a path) on the start screen
spacejunk ~/projects   # scan a path directly
spacejunk --scan / 20  # no TUI: print the 20 largest entries
```

The tree shows up immediately and re-sorts itself as sizes grow; folders not read yet show a
spinner. You can browse and open folders while the scan runs.

| Key | Action |
|---|---|
| `p` or `/` (disk list) | type a path to scan (`~` works) |
| `↑` `↓` / `j` `k` | move |
| `→` / `l` | open folder (or step into it) |
| `←` / `h` | close folder (or jump to the parent) |
| `Enter` / `Space` | open/close |
| `o` | show the selected folder or file in the file manager (Explorer, Finder, or the desktop's default on Linux) |
| `PgUp` `PgDn` `g` `G` | scroll / top / bottom |
| `r` | rescan |
| `Esc` | back to the disk list (cancels a running scan) |
| `q` | quit |

spacejunk **only reads**: nothing is ever deleted or changed.

## How sizes are counted

- Size = **real space used on disk** (allocated blocks, same as `du`), not the apparent size.
  On Windows, OneDrive files that live only in the cloud count as 0.
- Hard links are counted once (on Windows this keeps `WinSxS` from being counted twice); symlinks and junctions are not followed.
- Other filesystems mounted inside the target are not crossed (scanning `/` skips `/proc`, `/mnt/c`, ...).
- Folders you cannot read are counted under `errors`. Run with `sudo` (or as Administrator) to see everything.

## Performance

Parallel scan (rayon, 2 threads per core) with atomic counters flushed once per directory.
Measured with `hyperfine` on a home folder with ~826k entries (16 cores, warm cache):

| | time | memory |
|---|---|---|
| `du -s -x` | 3.03 s | 58 MiB |
| **spacejunk** | **0.23 s** (~13x) | 87 MiB |

About 90% of the time is spent in kernel syscalls (`statx`/`getdents`), so parallelism is what pays off.
The extra memory is the whole tree, kept for browsing. The scanner fills that tree directly
(atomic sizes propagated up to the root) and sorting only happens for the folders open on screen,
which made the live scan faster than the previous version that built the tree only at the end.

On Windows, the whole `C:\` drive (~490k files, 285 GiB) takes ~1.9 s.

> In WSL, Windows drives (`/mnt/c`) go through the 9p protocol and are much slower: use the native
> `spacejunk.exe` for them (`C:\Program Files`: 9 s through WSL, 0.05 s native).

## Development

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings   # clippy::pedantic is enabled in Cargo.toml
cargo build --profile profiling             # release with symbols, for perf/samply

# Windows from Linux/WSL (cargo install cargo-xwin; rustup target add x86_64-pc-windows-msvc)
cargo xwin clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
cargo xwin build --release --target x86_64-pc-windows-msvc
```

CI runs fmt, clippy and the tests on Linux, macOS and Windows.

### Releases

Automated with [release-plz](https://release-plz.dev) (`.github/workflows/release.yml`).
Use [Conventional Commits](https://www.conventionalcommits.org/):
`fix:` makes a patch, `feat:` a minor, `feat!:` a major. Every push to `main` opens/updates a release
PR with the new version and the CHANGELOG; merging it creates the tag, publishes the crate to crates.io
and attaches the Linux (x86_64/arm64, static), macOS (Intel/Apple Silicon) and Windows (x64/arm64)
binaries to the GitHub Release, each with its `.sha256`. The same run builds the Microsoft Store
bundle (`packaging/msix/`) as a workflow artifact, to upload in Partner Center.

Requires the `CARGO_REGISTRY_TOKEN` secret in the repository.

## License

MIT

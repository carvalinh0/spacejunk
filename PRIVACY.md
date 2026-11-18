# Privacy policy

Effective date: November 18, 2026 (updated for the `d` key, version 1.1.0)

spacejunk is a disk usage analyzer that runs on your computer. It does not collect, store or
share personal information, and it has no telemetry, analytics, ads or accounts.

## What spacejunk reads

To show how much space folders take, spacejunk reads the names and sizes of files and folders on
the drives or paths you choose. This information is only shown on your screen while the program
runs. It is never saved, and it never leaves your computer.

spacejunk changes nothing on its own. The only change it can make is the one you ask for: when you
press `d` and confirm, it moves that folder or file to the Recycle Bin (Windows) or the Trash
(macOS, Linux), from where you can restore it. It never deletes anything for good.

## Network access

- **Microsoft Store version:** spacejunk makes no network requests. Updates are handled by the
  Microsoft Store.
- **Other versions** (installed with `cargo install`, the install scripts or the GitHub release
  binaries): on startup, spacejunk asks crates.io for the latest version number, to tell you
  when an update is available. The request contains only the program name, its version and the
  address of this repository. Like any web request, it reaches crates.io from your IP address;
  crates.io is run by the Rust Foundation, see its
  [privacy policy](https://rustfoundation.org/policy/privacy-policy/).
  Nothing about your files or your computer is sent.

## Changes

Changes to this policy are published in this file, and its history is visible in the
repository.

## Contact

Questions about this policy: open an issue at https://github.com/carvalinh0/spacejunk/issues.

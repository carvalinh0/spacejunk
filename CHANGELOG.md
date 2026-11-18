# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.0](https://github.com/carvalinh0/spacejunk/compare/v1.0.1...v1.1.0) - 2026-11-18

### Added

- JSON output for --scan (--json) ([#18](https://github.com/carvalinh0/spacejunk/pull/18))
- mouse support ([#17](https://github.com/carvalinh0/spacejunk/pull/17))
- list the folders that could not be read (e) ([#16](https://github.com/carvalinh0/spacejunk/pull/16))
- largest files list (f) ([#15](https://github.com/carvalinh0/spacejunk/pull/15))
- show the selected folder or file in the file manager (o) ([#14](https://github.com/carvalinh0/spacejunk/pull/14))

### Fixed

- require Rust 1.95, the real minimum, and check it in CI ([#11](https://github.com/carvalinh0/spacejunk/pull/11))

### Other

- render whole screens with ratatui's TestBackend ([#12](https://github.com/carvalinh0/spacejunk/pull/12))

## [1.0.1](https://github.com/carvalinh0/spacejunk/compare/v1.0.0...v1.0.1) - 2026-11-18

### Other

- install from the Microsoft Store on Windows ([#10](https://github.com/carvalinh0/spacejunk/pull/10))
- Update README to remove output example and add image

## [1.0.0](https://github.com/carvalinh0/spacejunk/releases/tag/v1.0.0) - 2026-11-18

First stable release: the command line, the keys and the `--scan` output keep working the way
they do now. 1.0 is also the first version the Microsoft Store accepts (it refuses versions
starting with 0).

### Added

- Microsoft Store package (x64 and arm64), re-signed by Microsoft so Smart App Control lets it run

### Changed

- the Microsoft Store version does not show the update notice: the Store updates it

## [0.1.1](https://github.com/carvalinh0/spacejunk/releases/tag/v0.1.1) - 2026-11-18

No code changes. First release with prebuilt binaries attached, so the install
scripts work without Rust.

### Fixed

- release binaries are attached to a draft release before it is published
  (immutable releases refused them on v0.1.0)

### Other

- pin GitHub Actions to commit SHAs and scope workflow permissions
- add issue and pull request templates

## [0.1.0](https://github.com/carvalinh0/spacejunk/releases/tag/v0.1.0) - 2026-11-18

### Added

- live disk usage analyzer TUI: parallel scan, tree from heaviest to lightest, updated while scanning
- Linux, macOS and native Windows support
- disk picker, path prompt and headless `--scan` mode
- update notice when a newer version is on crates.io
- install scripts for Linux, macOS and Windows with app menu launchers

### Other

- add README and logo

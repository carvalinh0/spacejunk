# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

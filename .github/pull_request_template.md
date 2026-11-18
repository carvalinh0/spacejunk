<!--
The PR title becomes the commit on main and decides the next release:
  fix: ...   -> patch    feat: ...   -> minor    feat!: ...  -> major
  docs: / ci: / chore: / refactor: / test: / perf: are fine for everything else.
-->

## What and why

<!-- What does this change, and what problem does it solve? Link the issue: "Closes #123". -->

## How it was tested

<!-- Commands you ran, what you checked by hand in the TUI, on which systems. -->

- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test --all-targets` pass
- [ ] Tried it on: <!-- Linux / macOS / Windows -->

## Checklist

- [ ] New or changed keys, flags or behaviour are documented in `README.md` (and in the footer, for keys)
- [ ] Sizes are still counted the same way (real disk usage, hard links once, no symlinks followed), or the change explains why not
- [ ] Release asset names and install locations used by `scripts/install.*` are unchanged
- [ ] Performance changes include before/after numbers (`hyperfine` on `spacejunk --scan <dir>`)

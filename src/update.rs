//! Best-effort check for a newer release on crates.io, shown in the title bar.
//!
//! It shells out to the system `curl` (shipped with macOS, Windows 10+ and
//! nearly every Linux) instead of linking an HTTP/TLS crate: those pull in C
//! code (ring) that needs a C toolchain on every target, which would break
//! `cargo install` on a bare Windows. No curl, offline or slow: no notice,
//! never an error.

use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use crate::platform;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const CRATE_URL: &str = "https://crates.io/api/v1/crates/spacejunk";
/// crates.io asks automated clients to identify themselves.
const USER_AGENT: &str = concat!(
    "spacejunk/",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// Receives the newer version, if there is one. Runs in the background.
pub fn spawn_check() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    // The Microsoft Store updates its package by itself; "rerun the installer"
    // would put a second copy next to it.
    if platform::is_packaged() {
        return rx;
    }
    thread::spawn(move || {
        let latest = fetch().and_then(|body| parse_latest(&body).map(str::to_owned));
        if let Some(latest) = latest
            && is_newer(&latest, CURRENT)
        {
            let _ = tx.send(latest);
        }
    });
    rx
}

fn fetch() -> Option<String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "5", "-A", USER_AGENT, CRATE_URL])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `"max_stable_version":"x.y.z"` from the crates.io answer; a JSON parser
/// for one field is not worth a dependency.
fn parse_latest(body: &str) -> Option<&str> {
    const KEY: &str = "\"max_stable_version\":\"";
    let start = body.find(KEY)? + KEY.len();
    let len = body[start..].find('"')?;
    Some(&body[start..start + len])
}

/// `(major, minor, patch, is_release)`: a release sorts after its own
/// pre-releases (`0.2.0` > `0.2.0-rc.1`). Pre-releases are not ordered among
/// themselves; crates.io only answers with a stable version anyway.
fn numbers(version: &str) -> Option<(u64, u64, u64, bool)> {
    let core = version.split('+').next()?;
    let (core, is_release) = match core.split_once('-') {
        Some((core, _pre)) => (core, false),
        None => (core, true),
    };
    let mut parts = core.split('.').map(str::parse::<u64>);
    let mut next = || parts.next()?.ok();
    Some((next()?, next()?, next()?, is_release))
}

fn is_newer(latest: &str, current: &str) -> bool {
    matches!((numbers(latest), numbers(current)), (Some(l), Some(c)) if l > c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_max_stable_version() {
        let body = r#"{"crate":{"id":"spacejunk","max_version":"0.3.0-rc.1","max_stable_version":"0.2.1","name":"spacejunk"}}"#;
        assert_eq!(parse_latest(body), Some("0.2.1"));
        assert_eq!(parse_latest(r#"{"errors":[{"detail":"Not Found"}]}"#), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.10.0", "0.9.9"), "not a string comparison");
        assert!(is_newer("1.0.0", "0.99.0"));
        assert!(
            is_newer("0.2.0", "0.2.0-rc.1"),
            "release beats its pre-release"
        );
        assert!(!is_newer("0.2.0-rc.1", "0.2.0"));
        assert!(
            !is_newer("0.2.0", "0.2.0+build.5"),
            "build metadata is ignored"
        );
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}

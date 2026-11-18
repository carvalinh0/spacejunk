//! Folders that are safe to delete because the programs that made them
//! rebuild them: dependencies and build output, caches, temporary files.
//!
//! Only folders the user can delete: system ones (`C:\Windows\Temp`) would be
//! refused by `d`, and the trash itself is left out (moving the Recycle Bin to
//! the Recycle Bin makes no sense).

use std::path::Path;

use crate::scan::Node;

/// Folder names that are junk wherever they are.
const NAMES: [(&str, &str); 7] = [
    ("__pycache__", "Python bytecode"),
    (".pytest_cache", "pytest cache"),
    (".mypy_cache", "mypy cache"),
    (".ruff_cache", "Ruff cache"),
    (".tox", "tox environments"),
    ("DerivedData", "Xcode build"),
    (".cache", "cache"),
];

const PACKAGE_JSON: &[&str] = &["package.json"];
const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
];
const GRADLE_BUILD: &[&str] = &[
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// Folder names that are junk only inside a project that can rebuild them:
/// the folder next to them must hold one file of each list. A bare
/// `node_modules` can be part of an installed app (VS Code ships one), and
/// `~/.gradle` holds `gradle.properties`, credentials included.
const IN_PROJECTS: [(&str, &str, [&[&str]; 2]); 6] = [
    ("node_modules", "npm packages", [PACKAGE_JSON, LOCKFILES]),
    (".next", "Next.js build", [PACKAGE_JSON, PACKAGE_JSON]),
    (".nuxt", "Nuxt build", [PACKAGE_JSON, PACKAGE_JSON]),
    (
        ".parcel-cache",
        "Parcel cache",
        [PACKAGE_JSON, PACKAGE_JSON],
    ),
    (".turbo", "Turborepo cache", [PACKAGE_JSON, PACKAGE_JSON]),
    (".gradle", "Gradle cache", [GRADLE_BUILD, GRADLE_BUILD]),
];

/// Folders that are junk at these places, matched on the end of the path
/// (lowercase, `/` separators).
const PLACES: [(&str, &str); 3] = [
    ("appdata/local/temp", "temporary files"),
    ("library/caches", "app caches"),
    ("windows.old", "previous Windows install"),
];

/// The Cache Directory Tagging spec: a folder with this file is a cache
/// (Cargo's `target/`, among others).
const CACHE_TAG: &str = "CACHEDIR.TAG";

/// What kind of junk the folder at `path`, inside `parent`, is, if it is.
pub fn kind(dir: &Node, parent: &Node, path: &Path) -> Option<&'static str> {
    if !dir.is_dir {
        return None;
    }
    if let Some((_, what)) = NAMES.iter().find(|(name, _)| **name == *dir.name) {
        return Some(what);
    }
    let has = |files: &[&str]| {
        parent
            .children()
            .iter()
            .any(|c| !c.is_dir && files.contains(&&*c.name))
    };
    if let Some((_, what, needs)) = IN_PROJECTS.iter().find(|(name, ..)| **name == *dir.name) {
        return needs.iter().all(|files| has(files)).then_some(*what);
    }
    let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
    let at = |place: &str| path == place || path.ends_with(&format!("/{place}"));
    if let Some((_, what)) = PLACES.iter().find(|(place, _)| at(place)) {
        return Some(what);
    }
    let tagged = dir
        .children()
        .iter()
        .any(|c| !c.is_dir && &*c.name == CACHE_TAG);
    tagged.then_some("cache (CACHEDIR.TAG)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> Node {
        Node::fake_dir(name, Vec::new())
    }

    /// `kind` of a folder named `name` inside a folder holding `files`.
    fn kind_in(name: &str, files: &[&str]) -> Option<&'static str> {
        let parent = Node::fake_dir("p", files.iter().map(|f| Node::fake_file(f, 1)).collect());
        kind(&dir(name), &parent, &Path::new("/p").join(name))
    }

    #[test]
    fn known_names_anywhere() {
        assert_eq!(kind_in("__pycache__", &[]), Some("Python bytecode"));
        assert_eq!(kind_in("src", &[]), None);
        let file = Node::fake_file("__pycache__", 1);
        assert_eq!(
            kind(&file, &dir("p"), Path::new("/p/__pycache__")),
            None,
            "only folders"
        );
    }

    #[test]
    fn project_folders_need_the_project_next_to_them() {
        let npm = ["package.json", "package-lock.json"];
        assert_eq!(kind_in("node_modules", &npm), Some("npm packages"));
        assert_eq!(
            kind_in("node_modules", &["package.json"]),
            None,
            "no lockfile"
        );
        assert_eq!(kind_in("node_modules", &[]), None, "VS Code's own");
        assert_eq!(kind_in(".next", &["package.json"]), Some("Next.js build"));
        assert_eq!(
            kind_in(".gradle", &["settings.gradle.kts"]),
            Some("Gradle cache")
        );
        assert_eq!(
            kind_in(".gradle", &["gradle.properties"]),
            None,
            "~/.gradle"
        );
    }

    #[test]
    fn places_match_the_end_of_the_path_ignoring_case_and_separators() {
        let at = |name: &str, path: &str| kind(&dir(name), &dir("p"), Path::new(path));
        assert_eq!(
            at("Temp", r"C:\Users\Me\AppData\Local\Temp"),
            Some("temporary files")
        );
        assert_eq!(at("Temp", r"C:\Users\Me\Documents\Temp"), None);
        assert_eq!(at("Caches", "/Users/me/Library/Caches"), Some("app caches"));
        assert_eq!(
            at("Windows.old", r"C:\Windows.old"),
            Some("previous Windows install")
        );
        assert_eq!(at("notWindows.old", r"C:\notWindows.old"), None);
    }

    #[test]
    fn cache_tagged_folders() {
        let target = Node::fake_dir("target", vec![Node::fake_file(CACHE_TAG, 1)]);
        let p = Path::new("/home/me/app/target");
        assert_eq!(kind(&target, &dir("app"), p), Some("cache (CACHEDIR.TAG)"));
        assert_eq!(
            kind(&dir("target"), &dir("app"), p),
            None,
            "no tag, no guess"
        );
    }

    #[test]
    fn the_trash_and_system_folders_are_not_junk() {
        let at = |name: &str, path: &str| kind(&dir(name), &dir("p"), Path::new(path));
        assert_eq!(at("$Recycle.Bin", r"C:\$Recycle.Bin"), None);
        assert_eq!(at("Temp", r"C:\Windows\Temp"), None);
        assert_eq!(at("Trash", "/home/me/.local/share/Trash"), None);
    }
}

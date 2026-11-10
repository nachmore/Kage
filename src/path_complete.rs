//! Folder and file completion for the launcher: typing `~/Down` offers
//! `~/Downloads/`, so Enter opens it before the whole path is typed and Tab
//! fills it in to keep going.
//!
//! Only path-shaped input counts: `~`, `~/…`, an absolute path with at least
//! two separators (`/Users/…`; a lone `/word` is a slash command), or a
//! drive path on Windows (`C:\…`). Network paths (`\\server`, `//server`)
//! are left alone: listing one can stall for seconds. One directory is read,
//! never recursively, and both the entries scanned and the results returned
//! are capped.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Most completions offered at once.
pub const MAX_RESULTS: usize = 8;
/// Most directory entries read to find them (a huge folder stays cheap).
const MAX_SCAN: usize = 5_000;
/// Longest input treated as a path.
const MAX_INPUT: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathCompletion {
    /// The entry's name.
    pub name: String,
    /// What the input reads once completed, as the user typed it (`~`
    /// kept, their separator kept); folders end with a separator.
    pub display: String,
    /// The absolute path, for opening it.
    pub path: String,
    pub is_folder: bool,
}

/// A typed path split into the folder to list and the partial name in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedPath {
    /// The folder to list, absolute.
    pub dir: PathBuf,
    /// What the user typed of the entry's name (may be empty).
    pub partial: String,
    /// The input up to and including its last separator.
    pub display_base: String,
    /// The separator to end a completed folder with.
    pub sep: char,
}

fn is_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// Whether `input` is shaped like a path this module completes.
pub fn looks_like_path(input: &str) -> bool {
    let s = input.trim();
    if s.is_empty() || s.len() > MAX_INPUT || s.contains(['\n', '\r', '\0']) {
        return false;
    }
    if s.starts_with("//") || s.starts_with("\\\\") {
        return false;
    }
    if s == "~" || (s.starts_with('~') && s[1..].starts_with(is_sep)) {
        return true;
    }
    if cfg!(windows) {
        return is_drive_path(s);
    }
    s.starts_with('/') && s.matches('/').count() >= 2
}

/// Split `input` into the folder to list and the partial name, resolving
/// `~` against `home`. `None` when it isn't a completable path.
pub fn parse(input: &str, home: Option<&Path>) -> Option<TypedPath> {
    let s = input.trim();
    if !looks_like_path(s) {
        return None;
    }
    // `~` alone lists the home folder.
    let s_owned;
    let s = if s == "~" {
        s_owned = format!("~{}", std::path::MAIN_SEPARATOR);
        s_owned.as_str()
    } else {
        s
    };
    let last = s.rfind(is_sep)?;
    let sep = s[last..]
        .chars()
        .next()
        .unwrap_or(std::path::MAIN_SEPARATOR);
    let display_base = s[..=last].to_string();
    let partial = s[last + 1..].to_string();
    let dir_typed = &s[..=last];
    let dir = if let Some(rest) = dir_typed.strip_prefix('~') {
        let home = home?;
        let rest = rest.trim_start_matches(is_sep);
        if rest.is_empty() {
            home.to_path_buf()
        } else {
            home.join(rest)
        }
    } else {
        PathBuf::from(dir_typed)
    };
    Some(TypedPath {
        dir,
        partial,
        display_base,
        sep,
    })
}

fn matches_prefix(name: &str, partial: &str) -> bool {
    if partial.is_empty() {
        return true;
    }
    // Linux file systems are case-sensitive; the macOS and Windows defaults
    // aren't, and people type `~/down` there.
    if cfg!(target_os = "linux") {
        name.starts_with(partial)
    } else {
        name.to_lowercase().starts_with(&partial.to_lowercase())
    }
}

/// The entries of `typed.dir` that complete `typed.partial`: folders first,
/// then by name; hidden ones (`.name`) only when the partial starts with `.`.
pub fn list(typed: &TypedPath) -> Vec<PathCompletion> {
    let Ok(entries) = std::fs::read_dir(&typed.dir) else {
        return Vec::new();
    };
    let show_hidden = typed.partial.starts_with('.');
    let mut found: Vec<PathCompletion> = entries
        .take(MAX_SCAN)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (!show_hidden && name.starts_with('.')) || !matches_prefix(&name, &typed.partial) {
                return None;
            }
            let path = entry.path();
            // Follow a symlink to see whether it names a folder.
            let is_folder = std::fs::metadata(&path)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            let mut display = format!("{}{}", typed.display_base, name);
            if is_folder {
                display.push(typed.sep);
            }
            Some(PathCompletion {
                name,
                display,
                path: path.to_string_lossy().into_owned(),
                is_folder,
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.is_folder
            .cmp(&a.is_folder)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    found.truncate(MAX_RESULTS);
    found
}

/// Completions for `input`, or none when it isn't a path or the folder
/// can't be read.
pub fn complete(input: &str, home: Option<&Path>) -> Vec<PathCompletion> {
    parse(input, home).map(|t| list(&t)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_path_shaped_input_counts() {
        for yes in ["~", "~/", "~/Down", "~/Downloads/"] {
            assert!(looks_like_path(yes), "{yes}");
        }
        for no in [
            "",
            "hello",
            "~user",
            "//server/share",
            "\\\\server\\share",
            "~/a\nb",
            "what is ~/x",
        ] {
            assert!(!looks_like_path(no), "{no}");
        }
        if cfg!(windows) {
            assert!(looks_like_path("C:\\Users"));
            assert!(looks_like_path("d:/x"));
            assert!(!looks_like_path("/Users/x"));
        } else {
            assert!(looks_like_path("/Users/"));
            assert!(looks_like_path("/Applications/Sl"));
            // A lone `/word` is a slash command.
            assert!(!looks_like_path("/model"));
            assert!(!looks_like_path("/"));
        }
    }

    #[test]
    fn parse_resolves_home_and_keeps_what_was_typed() {
        let home = Path::new("/home/alex");
        let t = parse("~/Down", Some(home)).unwrap();
        assert_eq!(t.dir, PathBuf::from("/home/alex"));
        assert_eq!(t.partial, "Down");
        assert_eq!(t.display_base, "~/");
        let t = parse("~/Documents/Rep", Some(home)).unwrap();
        assert_eq!(t.dir, PathBuf::from("/home/alex/Documents/"));
        assert_eq!(t.partial, "Rep");
        assert_eq!(t.display_base, "~/Documents/");
        let t = parse("~", Some(home)).unwrap();
        assert_eq!(t.dir, PathBuf::from("/home/alex"));
        assert_eq!(t.partial, "");
        // No home folder known: nothing to resolve `~` against.
        assert_eq!(parse("~/Down", None), None);
    }

    #[test]
    fn lists_matching_entries_folders_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("Downloads")).unwrap();
        std::fs::create_dir(root.join("Documents")).unwrap();
        std::fs::write(root.join("download-notes.txt"), b"x").unwrap();
        std::fs::write(root.join("Desktop.ini"), b"x").unwrap();
        std::fs::create_dir(root.join(".dotfolder")).unwrap();

        let base = format!("~{}", std::path::MAIN_SEPARATOR);
        let got = complete(&format!("{base}Down"), Some(root));
        let names: Vec<_> = got.iter().map(|c| c.name.as_str()).collect();
        if cfg!(target_os = "linux") {
            assert_eq!(names, ["Downloads"]);
        } else {
            // Case-insensitive: the file matches too, after the folder.
            assert_eq!(names, ["Downloads", "download-notes.txt"]);
        }
        let first = &got[0];
        assert!(first.is_folder);
        assert_eq!(
            first.display,
            format!("{base}Downloads{}", std::path::MAIN_SEPARATOR)
        );
        assert_eq!(PathBuf::from(&first.path), root.join("Downloads"));

        // Hidden entries only when asked for.
        let all = complete(&base, Some(root));
        assert!(all.iter().all(|c| !c.name.starts_with('.')));
        let hidden = complete(&format!("{base}.d"), Some(root));
        assert_eq!(hidden.len(), 1);
        assert_eq!(hidden[0].name, ".dotfolder");
    }

    #[test]
    fn caps_results_and_ignores_missing_folders() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            std::fs::create_dir(dir.path().join(format!("folder{i:02}"))).unwrap();
        }
        let base = format!("~{}", std::path::MAIN_SEPARATOR);
        assert_eq!(
            complete(&format!("{base}fold"), Some(dir.path())).len(),
            MAX_RESULTS
        );
        assert!(complete(
            &format!("{base}nope{}x", std::path::MAIN_SEPARATOR),
            Some(dir.path())
        )
        .is_empty());
    }
}

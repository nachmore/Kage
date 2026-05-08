// macOS file search via Spotlight's `mdfind` CLI.
//
// `mdfind -interpret "<query>"` produces natural-language query results
// like the Spotlight search bar does — handles "kind:pdf filename",
// "created:today", and plain filename substrings uniformly. We scope to
// $HOME so system files, caches, and Xcode indexes don't dominate
// results.
//
// Each result is stat'ed for size + mtime. That's what makes this slow
// on large result sets — Spotlight itself is fast (milliseconds for
// index lookup), but stat-ing thousands of paths adds up. We cap at
// `max_results` with `mdfind`'s `-count` limitation handled in-process
// by truncating: `-count 100` exists but returns just a count, not paths.
//
// No new dependencies — shells out to system binaries available on
// every macOS since 10.4.

use crate::os::file_search::FileSearchResult;
use chrono::{DateTime, Local};
use log::{debug, warn};
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

pub fn search_files_impl(query: &str, max_results: usize) -> Vec<FileSearchResult> {
    let trimmed = query.trim();
    if trimmed.is_empty() || max_results == 0 {
        return vec![];
    }

    // Scope to the user's home by default. A future enhancement could
    // accept additional search roots from config, but the common case
    // (launcher-style search) is "my files" — we don't want to surface
    // /Library or /System entries.
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            warn!("file_search: no home directory — returning empty");
            return vec![];
        }
    };

    // `-interpret` lets users type the same things they'd put in
    // Spotlight (`kind:image vacation`, `created:yesterday`, plain
    // filename fragments). `-onlyin` restricts to the home tree.
    let output = match Command::new("mdfind")
        .arg("-interpret")
        .arg("-onlyin")
        .arg(&home)
        .arg(trimmed)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn!("file_search: mdfind failed to launch: {e}");
            return vec![];
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!(
            "file_search: mdfind exited with {} — {}",
            output.status,
            stderr.trim()
        );
        return vec![];
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let paths: Vec<&str> = stdout.lines().take(max_results).collect();

    let mut results: Vec<FileSearchResult> =
        paths.into_iter().filter_map(result_for_path).collect();

    // Sort by most recently modified — matches the Windows ranking and
    // what users expect from a launcher. `mdfind` returns results in
    // arbitrary order (index-internal).
    results.sort_by(|a, b| b.modified.cmp(&a.modified));
    results
}

fn result_for_path(path_str: &str) -> Option<FileSearchResult> {
    let path = Path::new(path_str);

    // Extract filename — for directories, the leaf name is what the user
    // sees in Finder. Paths without a leaf (impossible for mdfind output,
    // but be defensive) are skipped.
    let name = path.file_name()?.to_string_lossy().to_string();

    // Stat for size + mtime + file-vs-folder. Symlinks get dereferenced
    // by `metadata` so a shortcut in $HOME points to its target's
    // attributes — intentional: users searching for a file don't care
    // whether the hit is the file or a link to it.
    let metadata = path.metadata().ok()?;
    let is_folder = metadata.is_dir();
    // Directory size on macOS is usually 64/96 bytes (directory entry
    // overhead), not the recursive tree size. Reporting that would
    // mislead users, so we zero it out and let the UI render something
    // like "—" for folders.
    let size = if is_folder { 0 } else { metadata.len() };

    let modified = metadata
        .modified()
        .ok()
        .map(system_time_to_iso8601)
        .unwrap_or_default();

    Some(FileSearchResult {
        name,
        path: path_str.to_string(),
        is_folder,
        size,
        modified,
    })
}

fn system_time_to_iso8601(t: SystemTime) -> String {
    // Convert to a chrono DateTime<Local> for predictable ISO 8601
    // formatting. std::time::SystemTime doesn't have a formatter of its
    // own; `chrono` is already a project dep so this adds no new weight.
    let datetime: DateTime<Local> = t.into();
    datetime.to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn empty_query_returns_empty() {
        assert!(search_files_impl("", 10).is_empty());
        assert!(search_files_impl("   ", 10).is_empty());
    }

    #[test]
    fn zero_max_results_returns_empty() {
        assert!(search_files_impl("anything", 0).is_empty());
    }

    #[test]
    fn result_for_path_extracts_filename_and_size() {
        // Write a real temp file so metadata() succeeds — mdfind tests
        // can't run under `cargo test` without a Spotlight-indexed temp
        // location, but the per-path stat path is testable directly.
        let tmpdir = tempfile::tempdir().unwrap();
        let file_path = tmpdir.path().join("hello.txt");
        fs::write(&file_path, b"hello world").unwrap();

        let r = result_for_path(file_path.to_str().unwrap()).unwrap();
        assert_eq!(r.name, "hello.txt");
        assert_eq!(r.size, 11); // "hello world" = 11 bytes
        assert!(!r.is_folder);
        assert!(!r.modified.is_empty());
    }

    #[test]
    fn result_for_folder_reports_zero_size() {
        let tmpdir = tempfile::tempdir().unwrap();
        let r = result_for_path(tmpdir.path().to_str().unwrap()).unwrap();
        assert!(r.is_folder);
        assert_eq!(
            r.size, 0,
            "directory size should be reported as 0 — the inode-overhead \
             figure from metadata().len() is misleading to show users"
        );
    }

    #[test]
    fn system_time_to_iso8601_round_trip() {
        // Sanity: a known instant formats to something that contains the
        // year digit(s). Avoids hard-coding the exact format so timezone
        // differences don't flake the test.
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let s = system_time_to_iso8601(t);
        assert!(s.contains("2023"), "expected 2023 in formatted output: {s}");
    }
}

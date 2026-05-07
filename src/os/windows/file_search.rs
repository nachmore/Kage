//! Windows file search via the WinRT `Windows.Storage.Search` API.
//!
//! Pre-2026-05-07 this shelled out to PowerShell on every call —
//! `powershell.exe -Command "<script using ADODB.Connection / SystemIndex SQL>"`
//! — which spent ~200ms booting the PowerShell runtime per query before
//! the actual SystemIndex lookup ran. The chat's "search files" path was
//! routinely hitting that 200ms tax.
//!
//! `Windows.Storage.Search` is the same API Explorer uses internally.
//! It transparently consults the Windows Search Index for indexed
//! locations (Users, Documents, Desktop, OneDrive — fast, microseconds)
//! and falls back to a slower file enumeration only for non-indexed
//! locations. We pass the user's home directory as the root with
//! `FolderDepth::Deep` and `IndexerOption::UseIndexerWhenAvailable`,
//! which covers the same surface the previous `SystemIndex` SQL hit.
//!
//! No more PowerShell, no more SQL escaping (the input goes through
//! WinRT's `UserSearchFilter` which interprets it as AQS — no SQL
//! injection class to worry about).

use log::{info, warn};
use std::sync::OnceLock;

use windows::core::HSTRING;
use windows::Storage::Search::{CommonFileQuery, FolderDepth, IndexerOption, QueryOptions};
use windows::Storage::StorageFolder;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows_collections::IIterable;

use crate::os::file_search::FileSearchResult;

/// Maximum query length. The previous PowerShell implementation
/// capped at 256 chars to bound DoS risk; AQS doesn't have the same
/// concern but we keep a similar cap so a pathological input can't
/// produce a multi-megabyte query string.
const MAX_QUERY_LEN: usize = 256;

/// Initialize WinRT for this thread once. Tauri's `spawn_blocking`
/// thread pool reuses worker threads, so a `OnceLock` per-thread isn't
/// possible — but `RoInitialize` is idempotent on the same thread
/// (returns `S_FALSE` for the second call), and we tolerate the warning
/// once. The OnceLock here just suppresses repeated logs on workers
/// that have already initialized.
static RO_INIT_ATTEMPTED: OnceLock<()> = OnceLock::new();

fn ensure_winrt_initialized() {
    RO_INIT_ATTEMPTED.get_or_init(|| {
        // SAFETY: RoInitialize is the standard WinRT entry point. It's
        // safe to call on any thread; subsequent calls on the same
        // thread return S_FALSE without harm.
        let hr = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        if let Err(e) = hr {
            warn!(
                "[file_search] RoInitialize failed: {} — searches will likely fail",
                e
            );
        }
    });
    // Also cover the case where the worker thread is fresh and the
    // OnceLock has already fired — initialize on this thread too.
    // RoInitialize is reference-counted per thread; idempotent calls
    // are cheap.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
}

/// Sanitize and bound the query string. AQS handles its own escaping,
/// but we still strip control characters (UI hygiene) and cap length.
fn sanitize_query(query: &str) -> String {
    query
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_QUERY_LEN)
        .collect()
}

pub fn search_files_impl(query: &str, max_results: usize) -> Vec<FileSearchResult> {
    let cleaned = sanitize_query(query);
    if cleaned.trim().is_empty() {
        return vec![];
    }
    if max_results == 0 {
        return vec![];
    }

    ensure_winrt_initialized();

    match run_query(&cleaned, max_results) {
        Ok(results) => {
            info!(
                "[file_search] Found {} results for '{}'",
                results.len(),
                query
            );
            results
        }
        Err(e) => {
            warn!("[file_search] Query failed: {}", e);
            vec![]
        }
    }
}

fn run_query(query: &str, max_results: usize) -> windows::core::Result<Vec<FileSearchResult>> {
    // Root the search at the user's home directory. The Windows Search
    // Index covers Documents/Desktop/Downloads/Pictures/Music/Videos
    // and OneDrive by default — all under home — so a single deep
    // query against home reaches the same surface the previous
    // SystemIndex query did, without us having to enumerate folder
    // roots ourselves.
    let home = match dirs::home_dir() {
        Some(p) => p,
        None => return Ok(vec![]),
    };
    let home_str = home.to_string_lossy().replace('/', "\\");
    let root_path = HSTRING::from(home_str);

    let folder = StorageFolder::GetFolderFromPathAsync(&root_path)?.join()?;

    // FileTypeFilter — empty IIterable<HSTRING> means "all types".
    // CreateCommonFileQuery requires it; build one from an empty Vec.
    let empty_filter: IIterable<HSTRING> = IIterable::from(Vec::<HSTRING>::new());
    let options = QueryOptions::CreateCommonFileQuery(CommonFileQuery::OrderByDate, &empty_filter)?;

    options.SetUserSearchFilter(&HSTRING::from(query))?;
    options.SetFolderDepth(FolderDepth::Deep)?;
    options.SetIndexerOption(IndexerOption::UseIndexerWhenAvailable)?;

    let query_result = folder.CreateFileQueryWithOptions(&options)?;

    // GetFilesAsync(start, max) — the indexer respects this and stops
    // walking once it has the requested count. Saves walking 10k files
    // for a 10-result UI dropdown.
    let files = query_result.GetFilesAsync(0, max_results as u32)?.join()?;

    let count = files.Size()? as usize;
    let take = count.min(max_results);
    let mut out = Vec::with_capacity(take);

    for i in 0..take {
        let file = files.GetAt(i as u32)?;
        let name = file.Name()?.to_string_lossy();
        let path = file.Path()?.to_string_lossy();

        // Basic properties carry size + modified date. WinRT returns a
        // DateTime in 100-ns FILETIME ticks since 1601 — convert to
        // ISO 8601 to match the previous JSON shape.
        let (size, modified) = match read_basic_props(&file) {
            Ok((size, modified)) => (size, modified),
            Err(e) => {
                warn!(
                    "[file_search] failed to read properties for {}: {}",
                    path, e
                );
                (0, String::new())
            }
        };

        out.push(FileSearchResult {
            name,
            path,
            // Storage::Search file queries return only files (folders
            // surface through CreateFolderQuery instead). Always false.
            is_folder: false,
            size,
            modified,
        });
    }

    Ok(out)
}

/// Read size + modified date off a `StorageFile`. Split out so the
/// type of the `IAsyncOperation` is unambiguous to the borrow checker
/// (the inline `.and_then(|op| op.join())` pattern triggered an
/// inference failure under the IAsyncOperation generic).
fn read_basic_props(file: &windows::Storage::StorageFile) -> windows::core::Result<(u64, String)> {
    let props = file.GetBasicPropertiesAsync()?.join()?;
    let size = props.Size().unwrap_or(0);
    let modified = props
        .DateModified()
        .ok()
        .and_then(|dt| filetime_ticks_to_iso8601(dt.UniversalTime))
        .unwrap_or_default();
    Ok((size, modified))
}

/// Convert a WinRT `DateTime::UniversalTime` (100-ns FILETIME ticks
/// since 1601-01-01 UTC) to an ISO 8601 string. Returns `None` if
/// the value is out of range.
fn filetime_ticks_to_iso8601(ticks: i64) -> Option<String> {
    // 1601-01-01 UTC to 1970-01-01 UTC in 100-ns ticks:
    //   369 years × 365.25 days × 24 × 3600 × 10_000_000
    //   = 11_644_473_600_000_000_000 / 100ns granularity
    const FILETIME_EPOCH_TO_UNIX_TICKS: i64 = 11_644_473_600 * 10_000_000;
    let unix_ticks = ticks.checked_sub(FILETIME_EPOCH_TO_UNIX_TICKS)?;
    let secs = unix_ticks / 10_000_000;
    let nanos = ((unix_ticks % 10_000_000) * 100) as u32;
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)?;
    Some(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    //! Pure-helper tests. The full WinRT round-trip needs an actual
    //! Windows host with a populated Search Index, so we exercise the
    //! sanitizer and the FILETIME conversion in isolation.

    use super::*;

    #[test]
    fn sanitize_strips_control_characters() {
        assert_eq!(sanitize_query("hello\nworld"), "helloworld");
        assert_eq!(sanitize_query("tab\there"), "tabhere");
        assert_eq!(sanitize_query("null\0inside"), "nullinside");
    }

    #[test]
    fn sanitize_caps_length() {
        let input: String = "a".repeat(500);
        assert_eq!(sanitize_query(&input).len(), MAX_QUERY_LEN);
    }

    #[test]
    fn sanitize_passes_through_normal_input() {
        assert_eq!(sanitize_query("report 2026"), "report 2026");
        // AQS handles wildcards / quotes natively — no escaping needed
        // and the previous SQL-LIKE escaping that turned `O'Reilly`
        // into `O''Reilly` is gone.
        assert_eq!(sanitize_query("O'Reilly"), "O'Reilly");
        assert_eq!(sanitize_query("foo*bar"), "foo*bar");
    }

    #[test]
    fn filetime_epoch_maps_to_unix_epoch() {
        // Exactly 11_644_473_600 seconds × 10_000_000 ticks should land
        // on 1970-01-01T00:00:00Z.
        let unix_epoch_ticks = 11_644_473_600_i64 * 10_000_000;
        assert_eq!(
            filetime_ticks_to_iso8601(unix_epoch_ticks).as_deref(),
            Some("1970-01-01T00:00:00Z"),
        );
    }

    #[test]
    fn filetime_known_value_round_trips() {
        // 2025-05-07T12:00:00Z = 1746619200 unix seconds
        // = (1746619200 + 11644473600) × 10_000_000 ticks since 1601
        let ticks = (1_746_619_200_i64 + 11_644_473_600) * 10_000_000;
        assert_eq!(
            filetime_ticks_to_iso8601(ticks).as_deref(),
            Some("2025-05-07T12:00:00Z"),
        );
    }

    #[test]
    fn filetime_pre_unix_epoch_returns_some() {
        // FILETIME 0 (= 1601-01-01) maps to a pre-unix-epoch
        // chrono::DateTime, which is a perfectly valid representation
        // and round-trips cleanly. Pin the contract so a future
        // refactor doesn't accidentally start dropping these.
        let s = filetime_ticks_to_iso8601(0).expect("0 ticks must convert");
        assert!(s.starts_with("1601-01-01"), "got {s}");
    }
}

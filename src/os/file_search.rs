// Cross-platform file search abstraction.
//
// Uses the OS-native search index where available:
// - Windows: Windows Search Index (SystemIndex via OLE DB)
// - macOS: Spotlight (`mdfind -interpret -onlyin $HOME`)
// - Linux: locate/mlocate — stub for now
//
// Designed to be extensible — future backends (e.g. Everything SDK) can be
// added as alternative implementations behind a config flag.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct FileSearchResult {
    pub name: String,
    pub path: String,
    pub is_folder: bool,
    pub size: u64,
    pub modified: String, // ISO 8601
}

/// Search for files matching the query using the OS-native search index.
/// Returns up to `max_results` results, sorted by most recently modified.
pub fn search_files(query: &str, max_results: usize) -> Vec<FileSearchResult> {
    if query.trim().is_empty() {
        return vec![];
    }
    crate::os::platform::file_search::search_files_impl(query, max_results)
}

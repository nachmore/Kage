// macOS file search — stub.
//
// A real implementation would shell out to `mdfind` (Spotlight), parse
// the result paths, and stat each for size/mtime. Until that exists,
// return empty and warn once so the user understands why their query
// produced no results.

use crate::os::file_search::FileSearchResult;
use std::sync::OnceLock;

static WARNED: OnceLock<()> = OnceLock::new();

pub fn search_files_impl(_query: &str, _max_results: usize) -> Vec<FileSearchResult> {
    WARNED.get_or_init(|| {
        log::warn!(
            "file_search: macOS implementation not yet available — \
             returning empty results. Spotlight (mdfind) integration is a follow-up."
        );
    });
    vec![]
}

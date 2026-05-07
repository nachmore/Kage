// Linux file search — stub.
//
// A real implementation would shell out to `locate`/`mlocate` (or
// `plocate` on newer distros), or fall back to `find` for systems
// without an index. Until that exists, return empty and warn once.

use crate::os::file_search::FileSearchResult;
use std::sync::OnceLock;

static WARNED: OnceLock<()> = OnceLock::new();

pub fn search_files_impl(_query: &str, _max_results: usize) -> Vec<FileSearchResult> {
    WARNED.get_or_init(|| {
        log::warn!(
            "file_search: Linux implementation not yet available — \
             returning empty results. locate/mlocate integration is a follow-up."
        );
    });
    vec![]
}

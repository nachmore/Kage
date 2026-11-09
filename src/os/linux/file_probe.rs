//! Linux side of `os::file_probe` (docs/design/EXTENSION_FILE_TEXT.md §8).
//! Linux sync apps keep whole files on disk, so nothing is a placeholder.

use crate::os::file_probe::{open_regular_unix, Opened};
use std::fs::Metadata;
use std::path::Path;

pub fn open_regular_impl(path: &Path) -> std::io::Result<Opened> {
    open_regular_unix(path)
}

pub fn is_placeholder_impl(_path: &Path, _metadata: &Metadata) -> bool {
    false
}

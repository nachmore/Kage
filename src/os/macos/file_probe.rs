//! macOS side of `os::file_probe` (docs/design/EXTENSION_FILE_TEXT.md §8).
//! A dataless file (`SF_DATALESS` in `st_flags`) downloads on first read.

use crate::os::cloud_files::Residency;
use crate::os::file_probe::{open_regular_unix, Opened};
use std::fs::Metadata;
use std::os::macos::fs::MetadataExt;
use std::path::Path;

pub fn open_regular_impl(path: &Path) -> std::io::Result<Opened> {
    open_regular_unix(path)
}

pub fn is_placeholder_impl(_path: &Path, metadata: &Metadata) -> bool {
    crate::os::macos::cloud_files::residency_from_flags(metadata.st_flags())
        == Residency::Placeholder
}

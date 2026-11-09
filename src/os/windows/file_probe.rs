//! Windows side of `os::file_probe` (docs/design/EXTENSION_FILE_TEXT.md
//! §8). Opening never blocks on a pipe the way it can on Unix, so the
//! check after opening is enough. A Cloud Files placeholder (OneDrive
//! "Files On-Demand") carries a recall attribute and downloads on first
//! read.

use crate::os::cloud_files::Residency;
use crate::os::file_probe::Opened;
use std::fs::Metadata;
use std::os::windows::fs::MetadataExt;
use std::path::Path;

pub fn open_regular_impl(path: &Path) -> std::io::Result<Opened> {
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Ok(Opened::NotRegular);
    }
    Ok(Opened::File(file))
}

pub fn is_placeholder_impl(_path: &Path, metadata: &Metadata) -> bool {
    crate::os::windows::cloud_files::residency_from_attributes(metadata.file_attributes())
        == Residency::Placeholder
}

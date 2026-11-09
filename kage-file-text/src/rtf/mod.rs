//! RTF (EXTENSION_FILE_TEXT §3.7, §5.9). Built in WP-FT4.
//!
//! Until then every request is `unsupported`.

use crate::{Extracted, Failure, Job, WorkerErrorCode};

/// Reads an RTF file. Not built yet.
pub fn extract(_job: &Job<'_>) -> Result<Extracted, Failure> {
    Err(Failure::new(
        WorkerErrorCode::Unsupported,
        "rtf reader not built yet",
    ))
}

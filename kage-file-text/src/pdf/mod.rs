//! PDF (EXTENSION_FILE_TEXT §3.7, §5.9, §6.2). Built in WP-FT3.
//!
//! Until then every request is `unsupported`.

use crate::{Extracted, Failure, Job, WorkerErrorCode};

/// Reads a PDF file. Not built yet.
pub fn extract(_job: &Job<'_>) -> Result<Extracted, Failure> {
    Err(Failure::new(
        WorkerErrorCode::Unsupported,
        "pdf reader not built yet",
    ))
}

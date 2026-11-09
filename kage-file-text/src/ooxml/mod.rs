//! DOCX and PPTX (EXTENSION_FILE_TEXT §3.7, §5.9). Built in WP-FT2.
//!
//! Until then every request is `unsupported`. `crate::sniff` already
//! opens the package (`open_package`, `find_part`, `read_entry`) and
//! checks that its main part matches the extension.

use crate::{Extracted, Failure, Job, WorkerErrorCode};

/// Reads a DOCX or PPTX file. Not built yet.
pub fn extract(job: &Job<'_>) -> Result<Extracted, Failure> {
    Err(Failure::new(
        WorkerErrorCode::Unsupported,
        format!("{} reader not built yet", job.format.id()),
    ))
}

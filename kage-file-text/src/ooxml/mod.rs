//! DOCX and PPTX (EXTENSION_FILE_TEXT §3.7, §5.9).
//!
//! `crate::sniff` has already checked that the ZIP's main part matches
//! the extension. From there:
//! - `package`: the parts, found through their relationships, each
//!   inflated through a capped reader with a budget for the whole file.
//! - `xml`: a streaming reader that matches elements by local name (so
//!   Strict OOXML reads like Transitional) and caps the nesting depth.
//! - `docx` and `pptx`: the two readers, with `styles` for Word's
//!   headings, lists and hidden text.
//!
//! Every reader writes straight into the shared `Normalizer` and stops as
//! soon as it is full, so `max_chars` bounds the work as well as the
//! output.

mod docx;
mod package;
mod pptx;
mod styles;
mod xml;

use crate::{Extracted, Failure, Job, WorkerErrorCode};
use kage_core::file_text_wire::Format;

/// A paragraph's text goes to the normalizer in one piece, so NFC sees it
/// whole, unless it grows past this many bytes.
const FLUSH_BYTES: usize = 64 * 1024;

/// Reads a DOCX or PPTX file.
pub fn extract(job: &Job<'_>) -> Result<Extracted, Failure> {
    match job.format {
        Format::Docx => docx::extract(job),
        Format::Pptx => pptx::extract(job),
        other => Err(Failure::new(
            WorkerErrorCode::Unsupported,
            format!("{} isn't an OOXML format", other.id()),
        )),
    }
}

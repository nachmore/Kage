//! Loading a PDF (EXTENSION_FILE_TEXT §6.2 steps 1 and 5, encryption).
//!
//! `pdf-extract` works on `lopdf` 0.42 documents. They load with the
//! stream cap as a load filter (`streams::cap_object`), plus a pass for
//! the streams the filter didn't see.
//!
//! Encryption: 0.42 opens RC4 and AES files with the empty user password
//! while loading. One that needs a real user password stays encrypted:
//! `encrypted`. One with only an owner password is read; if its
//! permissions don't allow copying, the text gets `restricted`.
//!
//! Repair: a file 0.42 can't load gets one more try through `lopdf` 0.47
//! (with its own 32 MiB decompression limit), which rebuilds a broken
//! cross-reference table. That document is saved to memory and loaded
//! again with 0.42. `restricted` then comes from the 0.47 load.

use super::{caught, streams};
use crate::{Failure, WorkerErrorCode};
use kage_core::file_text_wire::MAX_PDF_STREAM_BYTES;
use pdf_extract::{Document, LoadOptions, Permissions};

/// A document ready for the page pass.
pub struct Loaded {
    pub doc: Document,
    /// The file asks readers not to copy its text.
    pub restricted: bool,
}

fn encrypted() -> Failure {
    Failure::new(WorkerErrorCode::Encrypted, "PDF needs a password")
}

/// Loads `bytes` with 0.42, or through the repair step.
pub fn load(bytes: &[u8]) -> Result<Loaded, Failure> {
    match load_042(bytes) {
        Some(doc) => {
            if doc.is_encrypted() {
                return Err(encrypted());
            }
            let restricted = doc
                .encryption_state
                .as_ref()
                .is_some_and(|s| !s.permissions().contains(Permissions::COPYABLE));
            Ok(Loaded { doc, restricted })
        }
        None => repair(bytes),
    }
}

/// A 0.42 load with the caps. `None` if it fails (or panics).
fn load_042(bytes: &[u8]) -> Option<Document> {
    let options = LoadOptions {
        filter: Some(streams::cap_object),
        ..LoadOptions::default()
    };
    let mut doc = caught(|| Document::load_mem_with_options(bytes, options))?.ok()?;
    let was_encrypted = doc.encryption_state.is_some();
    streams::cap_unchecked(&mut doc, was_encrypted);
    Some(doc)
}

fn repair(bytes: &[u8]) -> Result<Loaded, Failure> {
    let malformed = |detail: &str| Failure::new(WorkerErrorCode::Malformed, detail);
    let options = lopdf::LoadOptions {
        max_decompressed_size: Some(MAX_PDF_STREAM_BYTES as usize),
        ..lopdf::LoadOptions::default()
    };
    let mut doc = caught(|| lopdf::Document::load_mem_with_options(bytes, options))
        .and_then(Result::ok)
        .ok_or_else(|| malformed("PDF doesn't load, even with repair"))?;
    if doc.is_encrypted() {
        return Err(encrypted());
    }
    let restricted = doc
        .encryption_state
        .as_ref()
        .is_some_and(|s| !s.permissions().contains(lopdf::Permissions::COPYABLE));
    let mut saved = Vec::with_capacity(bytes.len());
    caught(|| doc.save_to(&mut saved))
        .and_then(Result::ok)
        .ok_or_else(|| malformed("repaired PDF doesn't save"))?;
    drop(doc);
    let doc = load_042(&saved).ok_or_else(|| malformed("repaired PDF doesn't load"))?;
    if doc.is_encrypted() {
        return Err(encrypted());
    }
    Ok(Loaded { doc, restricted })
}

//! HTML (EXTENSION_FILE_TEXT §3.7, §5.9). A second-release format: built
//! here, offered once `READS_P1_FORMATS` is on.
//!
//! The text a browser would show, as plain text with light Markdown:
//! `h1`–`h3` become `#`–`###` headings, `li` items start with `- `,
//! table cells are split by a tab and rows by a line break, and
//! paragraphs by a blank line. Left out: `script`, `style`, `noscript`,
//! `template`, `svg`, `iframe` and `datalist` content, comments, the
//! `head` (but its `title`), and elements with the `hidden` attribute or
//! an inline `display: none` (`hidden_text_skipped`). Nothing is fetched.
//! The title is the first `<title>`.
//!
//! Decoding: a BOM, else a `<meta charset>` in the first 1,024 bytes
//! (`charset`), else the plain text order (`plain::decode`): UTF-8, then
//! UTF-8 with a few bad bytes, then a `chardetng` guess, which is
//! Windows-1252 for Western text, the HTML standard's fallback.

mod charset;
mod rules;
mod sink;
mod writer;

use crate::plain::{self, Decoded};
use crate::{Extracted, Failure, Job};
use encoding_rs::Encoding;
use kage_core::file_text_wire::{Warning, WorkerErrorCode};

/// Decodes an HTML file's bytes.
pub fn decode(bytes: &[u8]) -> Decoded<'_> {
    if Encoding::for_bom(bytes).is_none() {
        if let Some(encoding) = charset::prescan(bytes) {
            let (text, replaced) = encoding.decode_without_bom_handling(bytes);
            return Decoded {
                text,
                replaced,
                legacy: false,
            };
        }
    }
    plain::decode(bytes)
}

/// Reads an HTML file.
pub fn extract(job: &Job<'_>) -> Result<Extracted, Failure> {
    let decoded = decode(job.bytes);
    if plain::looks_binary(&decoded.text) {
        return Err(Failure::new(
            WorkerErrorCode::NotText,
            "control characters in the first 8 KiB",
        ));
    }
    let found = sink::read(&decoded.text, job.max_chars);
    let warnings = [
        (decoded.legacy, Warning::LegacyEncoding),
        (decoded.replaced, Warning::ReplacedCharacters),
        (found.hidden_text, Warning::HiddenTextSkipped),
    ]
    .into_iter()
    .filter_map(|(on, warning)| on.then_some(warning))
    .collect();
    Ok(Extracted {
        text: found.text,
        title: found.title,
        pages: None,
        warnings,
    })
}

#[cfg(test)]
mod tests;

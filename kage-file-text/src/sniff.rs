//! Format detection from the bytes (EXTENSION_FILE_TEXT §5.8).
//!
//! | Content | Detected as |
//! |---|---|
//! | OLE compound file | `encrypted` with an `EncryptedPackage` stream, else `legacy_format` |
//! | `%PDF-` in the first 1,024 bytes | `pdf` |
//! | ZIP whose `[Content_Types].xml` names a WordprocessingML main part | `docx` |
//! | ZIP whose `[Content_Types].xml` names a PresentationML main part | `pptx` |
//! | `{\rtf` at the start (after whitespace) | `rtf` |
//! | anything else | text, Markdown or HTML, by extension |
//!
//! If the content isn't what the extension implies: `wrong_content`. For
//! the text family the PDF header has to be at the very start, so a note
//! that mentions `%PDF-` near its top is still text. Whether the rest
//! decodes as text is the text reader's call (`not_text`).

use crate::{Failure, WorkerErrorCode};
use kage_core::file_text_wire::{Format, MAX_PART_INFLATED_BYTES, MAX_ZIP_ENTRIES};
use std::io::{Cursor, Read};

const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const PDF_HEADER: &[u8] = b"%PDF-";
const PDF_HEADER_WINDOW: usize = 1024;
const ZIP_LOCAL: &[u8] = b"PK\x03\x04";
const ZIP_EMPTY: &[u8] = b"PK\x05\x06";
const RTF_START: &[u8] = b"{\\rtf";
const CONTENT_TYPES: &str = "[Content_Types].xml";

/// Main-part content types of WordprocessingML (document, template,
/// with and without macros; Strict uses the same ones).
const DOCX_MAIN: &[&str] = &[
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
    "application/vnd.ms-word.document.macroenabled.main+xml",
    "application/vnd.ms-word.template.macroenabledtemplate.main+xml",
];

/// Main-part content types of PresentationML (presentation, slide show,
/// template, with and without macros).
const PPTX_MAIN: &[&str] = &[
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
    "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml",
    "application/vnd.openxmlformats-officedocument.presentationml.template.main+xml",
    "application/vnd.ms-powerpoint.presentation.macroenabled.main+xml",
    "application/vnd.ms-powerpoint.slideshow.macroenabled.main+xml",
    "application/vnd.ms-powerpoint.template.macroenabled.main+xml",
];

fn fail(code: WorkerErrorCode, detail: &str) -> Failure {
    Failure::new(code, detail)
}

/// Checks that `bytes` hold what an `expected` file holds.
pub fn check(expected: Format, bytes: &[u8]) -> Result<(), Failure> {
    if bytes.starts_with(&OLE_MAGIC) {
        return Err(if contains_utf16le(bytes, "EncryptedPackage") {
            fail(
                WorkerErrorCode::Encrypted,
                "OLE file with an EncryptedPackage stream",
            )
        } else {
            fail(WorkerErrorCode::LegacyFormat, "OLE compound file")
        });
    }
    let wrong = || {
        fail(
            WorkerErrorCode::WrongContent,
            "content doesn't match the extension",
        )
    };
    match expected {
        Format::Pdf => is_pdf(bytes, PDF_HEADER_WINDOW)
            .then_some(())
            .ok_or_else(wrong),
        Format::Docx | Format::Pptx => {
            if !is_zip(bytes) {
                return Err(wrong());
            }
            match ooxml_kind(bytes)? {
                Some(kind) if kind == expected => Ok(()),
                _ => Err(wrong()),
            }
        }
        Format::Rtf => is_rtf(bytes).then_some(()).ok_or_else(wrong),
        Format::Text | Format::Markdown | Format::Html => {
            if is_pdf(bytes, PDF_HEADER.len()) || is_zip(bytes) || is_rtf(bytes) {
                Err(wrong())
            } else {
                Ok(())
            }
        }
    }
}

/// `%PDF-` starts within the first `window` bytes.
pub fn is_pdf(bytes: &[u8], window: usize) -> bool {
    let head = &bytes[..bytes.len().min(window)];
    memchr::memmem::find(head, PDF_HEADER).is_some()
}

/// A ZIP archive: a local file header or an empty archive's end record.
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(ZIP_LOCAL) || bytes.starts_with(ZIP_EMPTY)
}

/// `{\rtf` after optional ASCII whitespace.
pub fn is_rtf(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    bytes[start..].starts_with(RTF_START)
}

fn contains_utf16le(bytes: &[u8], needle: &str) -> bool {
    let pattern: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
    memchr::memmem::find(bytes, &pattern).is_some()
}

/// The number of entries the end of central directory record declares,
/// read before the archive is parsed so a huge directory is refused
/// without building it.
pub fn declared_zip_entries(bytes: &[u8]) -> Option<u64> {
    const EOCD: &[u8] = b"PK\x05\x06";
    const LOCATOR: &[u8] = b"PK\x06\x07";
    const ZIP64_EOCD: &[u8] = b"PK\x06\x06";
    let last = bytes.len().checked_sub(22)?;
    let first = last.saturating_sub(u16::MAX as usize);
    let at = (first..=last).rev().find(|&i| &bytes[i..i + 4] == EOCD)?;
    let total = u16::from_le_bytes([bytes[at + 10], bytes[at + 11]]);
    if total != u16::MAX {
        return Some(u64::from(total));
    }
    let locator = at.checked_sub(20)?;
    if &bytes[locator..locator + 4] != LOCATOR {
        return Some(u64::from(total));
    }
    let offset = u64::from_le_bytes(bytes[locator + 8..locator + 16].try_into().ok()?);
    let offset = usize::try_from(offset).ok()?;
    let record = bytes.get(offset..offset.checked_add(40)?)?;
    if &record[..4] != ZIP64_EOCD {
        return Some(u64::from(total));
    }
    Some(u64::from_le_bytes(record[32..40].try_into().ok()?))
}

/// An open OOXML package. More than `MAX_ZIP_ENTRIES` entries is
/// `too_complex`; a ZIP that doesn't open is `malformed`.
pub fn open_package(bytes: &[u8]) -> Result<zip::ZipArchive<Cursor<&[u8]>>, Failure> {
    if declared_zip_entries(bytes).is_some_and(|n| n > MAX_ZIP_ENTRIES as u64) {
        return Err(fail(WorkerErrorCode::TooComplex, "too many ZIP entries"));
    }
    let archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| fail(WorkerErrorCode::Malformed, "ZIP archive doesn't open"))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(fail(WorkerErrorCode::TooComplex, "too many ZIP entries"));
    }
    Ok(archive)
}

/// The index of a part, matching its name without regard to ASCII case
/// (OPC part names are case-insensitive).
pub fn find_part<R: Read + std::io::Seek>(
    archive: &zip::ZipArchive<R>,
    name: &str,
) -> Option<usize> {
    archive.index_for_name(name).or_else(|| {
        (0..archive.len()).find(|&i| {
            archive
                .name_for_index(i)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
    })
}

/// Inflates one entry, at most `cap` bytes. Past the cap: `too_complex`.
/// Past its declared size: `malformed`. Encrypted: `encrypted`.
pub fn read_entry<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    cap: u64,
) -> Result<Vec<u8>, Failure> {
    let entry = archive.by_index(index).map_err(|e| match e {
        zip::result::ZipError::UnsupportedArchive(msg)
            if msg == zip::result::ZipError::PASSWORD_REQUIRED =>
        {
            fail(WorkerErrorCode::Encrypted, "encrypted ZIP entry")
        }
        _ => fail(WorkerErrorCode::Malformed, "ZIP entry doesn't open"),
    })?;
    let declared = entry.size();
    let limit = declared.min(cap);
    let mut out = Vec::with_capacity(limit.min(1 << 20) as usize);
    entry
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|_| fail(WorkerErrorCode::Malformed, "ZIP entry doesn't inflate"))?;
    let read = out.len() as u64;
    if read > declared {
        return Err(fail(
            WorkerErrorCode::Malformed,
            "ZIP entry inflates past its size",
        ));
    }
    if read > cap {
        return Err(fail(WorkerErrorCode::TooComplex, "ZIP entry too large"));
    }
    Ok(out)
}

/// XML bytes as UTF-8: a UTF-16 BOM is honoured, a UTF-8 BOM dropped.
pub fn xml_text(bytes: &[u8]) -> Result<std::borrow::Cow<'_, str>, Failure> {
    let malformed = || fail(WorkerErrorCode::Malformed, "XML part isn't text");
    match encoding_rs::Encoding::for_bom(bytes) {
        Some((encoding, bom)) => {
            let (text, had_errors) = encoding.decode_without_bom_handling(&bytes[bom..]);
            if had_errors {
                return Err(malformed());
            }
            Ok(text)
        }
        None => std::str::from_utf8(bytes)
            .map(std::borrow::Cow::Borrowed)
            .map_err(|_| malformed()),
    }
}

/// Which OOXML document a ZIP is, from `[Content_Types].xml`. `None`: a
/// ZIP, but not a Word or PowerPoint file.
pub fn ooxml_kind(bytes: &[u8]) -> Result<Option<Format>, Failure> {
    let mut archive = open_package(bytes)?;
    let Some(index) = find_part(&archive, CONTENT_TYPES) else {
        return Ok(None);
    };
    let xml = read_entry(&mut archive, index, MAX_PART_INFLATED_BYTES)?;
    let xml = xml_text(&xml)?;
    let mut reader = quick_xml::Reader::from_str(&xml);
    loop {
        use quick_xml::events::Event;
        let event = reader.read_event().map_err(|_| {
            fail(
                WorkerErrorCode::Malformed,
                "content types XML doesn't parse",
            )
        })?;
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let local = e.local_name();
                if local.as_ref() != b"Override" && local.as_ref() != b"Default" {
                    continue;
                }
                for attr in e.attributes().flatten() {
                    if attr.key.local_name().as_ref() != b"ContentType" {
                        continue;
                    }
                    let Ok(value) = attr.unescape_value() else {
                        continue;
                    };
                    let value = value.trim().to_ascii_lowercase();
                    if DOCX_MAIN.contains(&value.as_str()) {
                        return Ok(Some(Format::Docx));
                    }
                    if PPTX_MAIN.contains(&value.as_str()) {
                        return Ok(Some(Format::Pptx));
                    }
                }
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "sniff_tests.rs"]
mod tests;

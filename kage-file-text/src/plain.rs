//! Plain text and Markdown (EXTENSION_FILE_TEXT §5.9).
//!
//! Decoding, in this order:
//! 1. A BOM (UTF-8, UTF-16 LE or BE).
//! 2. UTF-16 without a BOM, when the NUL bytes in the first 8 KiB fall on
//!    every other byte. Checked before UTF-8, because UTF-16 text in the
//!    ASCII range is also valid UTF-8 (full of NULs).
//! 3. Strict UTF-8.
//! 4. UTF-8 with a few bad sequences (at most one per 20 valid multi-byte
//!    characters, or only a cut-off character at the end): decoded with
//!    U+FFFD (`replaced_characters`), so one bad byte doesn't turn a whole
//!    UTF-8 file into a legacy guess.
//! 5. Otherwise `chardetng` guesses a legacy encoding (`legacy_encoding`).
//!
//! Then more than 1 % control characters in the first 8 KiB means
//! `not_text`. Tab, line breaks, form feed and ESC (colour codes in
//! logs) don't count.
//!
//! Markdown's title: the YAML front matter's `title:`, else the first
//! level-1 heading (`# Title` or `Title` over `===`) outside code fences.

use crate::normalize::Normalizer;
use crate::{Extracted, Failure, Job};
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
use kage_core::file_text_wire::{Format, Warning, WorkerErrorCode};
use std::borrow::Cow;

/// The part of the input the NUL-pattern and control checks look at.
const SAMPLE: usize = 8 * 1024;
/// Most bytes handed to the encoding detector.
const DETECT_BYTES: usize = 1 << 20;
/// Front matter lines searched for `title:`.
const FRONT_MATTER_LINES: usize = 100;

/// Decoded text and how it was decoded.
pub struct Decoded<'a> {
    pub text: Cow<'a, str>,
    /// Some bytes became U+FFFD.
    pub replaced: bool,
    /// The encoding was guessed.
    pub legacy: bool,
}

/// Decodes bytes as text, following the order in the module docs.
pub fn decode(bytes: &[u8]) -> Decoded<'_> {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        return with(encoding, &bytes[bom_len..], false);
    }
    if let Some(encoding) = utf16_by_nul_pattern(bytes) {
        return with(encoding, bytes, false);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Decoded {
            text: Cow::Borrowed(text),
            replaced: false,
            legacy: false,
        };
    }
    if mostly_utf8(bytes) {
        return with(UTF_8, bytes, false);
    }
    let sample = &bytes[..bytes.len().min(DETECT_BYTES)];
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(sample, sample.len() == bytes.len());
    with(detector.guess(None, false), bytes, true)
}

fn with<'a>(encoding: &'static Encoding, bytes: &'a [u8], legacy: bool) -> Decoded<'a> {
    let (text, replaced) = encoding.decode_without_bom_handling(bytes);
    Decoded {
        text,
        replaced,
        legacy,
    }
}

fn utf16_by_nul_pattern(bytes: &[u8]) -> Option<&'static Encoding> {
    let sample = &bytes[..bytes.len().min(SAMPLE)];
    let pairs = sample.len() / 2;
    if pairs < 2 {
        return None;
    }
    let (mut even, mut odd) = (0usize, 0usize);
    for [first, second] in sample.as_chunks::<2>().0 {
        even += usize::from(*first == 0);
        odd += usize::from(*second == 0);
    }
    // Text in the ASCII range has a zero high byte in every unit: at odd
    // offsets in little endian, at even ones in big endian.
    if odd * 10 >= pairs * 3 && even * 20 <= pairs {
        Some(UTF_16LE)
    } else if even * 10 >= pairs * 3 && odd * 20 <= pairs {
        Some(UTF_16BE)
    } else {
        None
    }
}

fn lead_bytes(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b >= 0xC0).count()
}

fn mostly_utf8(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    let mut invalid = 0usize;
    let mut multibyte = 0usize;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                multibyte += lead_bytes(valid.as_bytes());
                break;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                multibyte += lead_bytes(valid);
                match e.error_len() {
                    Some(len) => {
                        invalid += 1;
                        rest = &after[len..];
                    }
                    // Only a character cut off at the end.
                    None => break,
                }
            }
        }
    }
    invalid == 0 || (multibyte > 0 && invalid * 20 <= multibyte)
}

fn counts_as_binary(c: char) -> bool {
    matches!(
        c as u32,
        0x00..=0x08 | 0x0E..=0x1A | 0x1C..=0x1F | 0x7F..=0x9F
    )
}

/// More than 1 % binary-looking characters in the first 8 KiB.
pub fn looks_binary(text: &str) -> bool {
    let (mut seen, mut bad) = (0usize, 0usize);
    for c in text.chars().take(SAMPLE) {
        seen += 1;
        bad += usize::from(counts_as_binary(c));
    }
    bad * 100 > seen
}

/// Reads a text or Markdown file.
pub fn extract(job: &Job<'_>) -> Result<Extracted, Failure> {
    let decoded = decode(job.bytes);
    if looks_binary(&decoded.text) {
        return Err(Failure::new(
            WorkerErrorCode::NotText,
            "control characters in the first 8 KiB",
        ));
    }
    let mut normalizer = Normalizer::new(job.max_chars).counting_rest();
    normalizer.push(&decoded.text);
    let text = normalizer.finish(true);
    let title = match job.format {
        Format::Markdown => markdown_title(&decoded.text),
        _ => None,
    };
    let mut warnings = Vec::new();
    if decoded.legacy {
        warnings.push(Warning::LegacyEncoding);
    }
    if decoded.replaced {
        warnings.push(Warning::ReplacedCharacters);
    }
    Ok(Extracted {
        text,
        title,
        pages: None,
        warnings,
    })
}

/// The title of a Markdown document, raw (the caller cleans it).
pub fn markdown_title(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut start = 0;
    if lines.first().is_some_and(|l| l.trim_end() == "---") {
        let end = lines
            .iter()
            .enumerate()
            .skip(1)
            .take(FRONT_MATTER_LINES)
            .find(|(_, l)| matches!(l.trim_end(), "---" | "..."))
            .map(|(i, _)| i);
        if let Some(end) = end {
            if let Some(title) = lines[1..end].iter().find_map(|l| front_matter_title(l)) {
                return Some(title);
            }
            start = end + 1;
        }
    }
    first_heading(&lines[start..])
}

fn front_matter_title(line: &str) -> Option<String> {
    let value = line.strip_prefix("title:")?.trim();
    let value = ['"', '\'']
        .iter()
        .find_map(|&q| value.strip_prefix(q).and_then(|v| v.strip_suffix(q)))
        .unwrap_or(value)
        .trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn run_of(s: &str, c: char) -> usize {
    s.chars().take_while(|&x| x == c).count()
}

fn first_heading(lines: &[&str]) -> Option<String> {
    let mut fence: Option<(char, usize)> = None;
    let mut previous: Option<&str> = None;
    for &line in lines {
        let body = line.trim_start_matches(' ');
        let indent = line.len() - body.len();
        if let Some((mark, len)) = fence {
            if indent <= 3
                && run_of(body, mark) >= len
                && body.trim_start_matches(mark).trim().is_empty()
            {
                fence = None;
            }
            continue;
        }
        if indent <= 3 {
            for mark in ['`', '~'] {
                let len = run_of(body, mark);
                if len >= 3 {
                    fence = Some((mark, len));
                }
            }
            if fence.is_some() {
                previous = None;
                continue;
            }
            if let Some(title) = atx_h1(body) {
                return Some(title);
            }
            let underline = body.trim_end();
            if !underline.is_empty() && underline.chars().all(|c| c == '=') {
                if let Some(p) = previous {
                    return Some(p.trim().to_string());
                }
            }
        }
        previous = (!line.trim().is_empty()).then_some(line);
    }
    None
}

fn atx_h1(body: &str) -> Option<String> {
    let rest = body.strip_prefix('#')?;
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let mut title = rest.trim();
    let open = title.trim_end_matches('#');
    if open.len() < title.len() && (open.is_empty() || open.ends_with([' ', '\t'])) {
        title = open.trim_end();
    }
    (!title.is_empty()).then(|| title.to_string())
}

#[cfg(test)]
#[path = "plain_tests.rs"]
mod tests;

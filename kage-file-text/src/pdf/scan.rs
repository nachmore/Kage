//! Light scans of PDF content and CMap syntax, run before `pdf-extract`
//! sees the file.
//!
//! - [`content`]: which XObjects a content stream draws with `Do`, and
//!   how often (for the form guard, `guard.rs`); and, on request, which
//!   two-byte codes it shows with each font name (for the CJK maps,
//!   `fonts.rs`). `q` and `Q` save and restore the font as they do in
//!   `pdf-extract`.
//! - [`max_nesting`]: how deep strings, arrays, dictionaries and
//!   procedures nest. The CMap and Type 1 parsers `pdf-extract` uses
//!   recurse once per level, so a file can overflow the stack with a
//!   few kilobytes of brackets (`fonts.rs`).
//!
//! Both are lexers only: they never fail, and on odd input they can only
//! over- or under-count.

use std::collections::{BTreeSet, HashMap};

/// Font names and saved states kept at most per stream.
const MAX_STACK: usize = 4096;

fn is_space(b: u8) -> bool {
    matches!(b, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(b: u8) -> bool {
    !is_space(b) && !is_delimiter(b)
}

fn skip_while(bytes: &[u8], mut i: usize, f: impl Fn(u8) -> bool) -> usize {
    while i < bytes.len() && f(bytes[i]) {
        i += 1;
    }
    i
}

/// A literal string starting after its `(` at `i`: its bytes and where
/// it ends.
fn literal(bytes: &[u8], mut i: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut depth = 1usize;
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        match b {
            b'\\' => {
                let Some(&next) = bytes.get(i) else { break };
                i += 1;
                match next {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0C),
                    b'0'..=b'7' => {
                        let mut value = u32::from(next - b'0');
                        for _ in 0..2 {
                            match bytes.get(i) {
                                Some(&d @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(value as u8);
                    }
                    b'\r' => {
                        if bytes.get(i) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    b'\n' => {}
                    other => out.push(other),
                }
            }
            b'(' => {
                depth += 1;
                out.push(b);
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return (out, i);
                }
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    (out, bytes.len())
}

/// A hex string starting after its `<` at `i`.
fn hex(bytes: &[u8], i: usize) -> (Vec<u8>, usize) {
    let end = skip_while(bytes, i, |b| b != b'>');
    let digits: Vec<u8> = bytes[i..end]
        .iter()
        .filter_map(|&b| (b as char).to_digit(16).map(|d| d as u8))
        .collect();
    let out = digits
        .chunks(2)
        .map(|pair| pair[0] << 4 | pair.get(1).copied().unwrap_or(0))
        .collect();
    (out, (end + 1).min(bytes.len()))
}

/// A name's text with `#xx` escapes decoded, as `lopdf` stores the keys.
fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = raw
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (raw[i], hex) {
            (b'#', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// After an inline image's `ID`: the end of its data, at the next `EI`
/// with white space before it and white space, a delimiter or the end
/// after it.
fn skip_inline_image(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while i + 1 < bytes.len() {
        let before = i == from || is_space(bytes[i - 1]);
        let after = bytes.get(i + 2).is_none_or(|&b| !is_regular(b));
        if before && after && &bytes[i..i + 2] == b"EI" {
            return i + 2;
        }
        i += 1;
    }
    bytes.len()
}

/// What a content stream draws and shows.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    /// XObject names drawn with `Do`, and how often.
    pub calls: HashMap<Vec<u8>, u64>,
    /// Two-byte codes shown with each font name (when asked for).
    pub shown: HashMap<Vec<u8>, BTreeSet<u16>>,
}

/// Scans content (module docs). `codes`: also record the codes shown.
pub fn content(bytes: &[u8], codes: bool) -> Scan {
    let mut scan = Scan::default();
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut strings: Vec<Vec<u8>> = Vec::new();
    let mut font: Option<Vec<u8>> = None;
    let mut saved: Vec<Option<Vec<u8>>> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if is_space(b) {
            i += 1;
            continue;
        }
        match b {
            b'%' => i = skip_while(bytes, i, |b| b != b'\n' && b != b'\r'),
            // `<<` and `>>`: a dictionary operand's brackets.
            b'<' | b'>' if bytes.get(i + 1) == Some(&b) => i += 2,
            b'(' | b'<' => {
                let (s, end) = if b == b'(' {
                    literal(bytes, i + 1)
                } else {
                    hex(bytes, i + 1)
                };
                if codes && strings.len() < MAX_STACK {
                    strings.push(s);
                }
                i = end;
            }
            b'/' => {
                let end = skip_while(bytes, i + 1, is_regular);
                if names.len() < MAX_STACK {
                    names.push(decode_name(&bytes[i + 1..end]));
                }
                i = end;
            }
            b if is_regular(b) => {
                let end = skip_while(bytes, i, is_regular);
                let op = &bytes[i..end];
                i = end;
                match op {
                    // `pdf-extract` takes the first operand of both.
                    b"Do" => {
                        if let Some(name) = names.first() {
                            *scan.calls.entry(name.clone()).or_default() += 1;
                        }
                    }
                    b"Tf" => font = names.first().cloned().or(font),
                    b"q" if saved.len() < MAX_STACK => saved.push(font.clone()),
                    b"Q" => font = saved.pop().unwrap_or(font),
                    b"Tj" | b"TJ" | b"'" | b"\"" => {
                        if let Some(name) = &font {
                            let set = scan.shown.entry(name.clone()).or_default();
                            for s in &strings {
                                set.extend(
                                    s.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]])),
                                );
                            }
                        }
                    }
                    b"ID" => i = skip_inline_image(bytes, (end + 1).min(bytes.len())),
                    // A number is an operand, not an operator.
                    _ if op[0].is_ascii_digit() || matches!(op[0], b'-' | b'+' | b'.') => continue,
                    _ => {}
                }
                names.clear();
                strings.clear();
            }
            // `[`, `]`, `{`, `}` and stray `>`: structure, not operands.
            _ => i += 1,
        }
    }
    scan
}

/// The deepest nesting of `(…)`, `[…]`, `{…}` and `<<…>>` together.
pub fn max_nesting(bytes: &[u8]) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let mut in_string = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string > 0 {
            match b {
                b'\\' => i += 1,
                b'(' => {
                    in_string += 1;
                    deepest = deepest.max(depth + in_string);
                }
                b')' => in_string -= 1,
                _ => {}
            }
            i += 1;
            continue;
        }
        match b {
            b'%' => {
                i = skip_while(bytes, i, |b| b != b'\n' && b != b'\r');
                continue;
            }
            b'(' => {
                in_string = 1;
                deepest = deepest.max(depth + 1);
            }
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            b'<' if bytes.get(i + 1) == Some(&b'<') => {
                depth += 1;
                deepest = deepest.max(depth);
                i += 1;
            }
            b'>' if bytes.get(i + 1) == Some(&b'>') => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    deepest
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;

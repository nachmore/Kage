//! Stream caps (EXTENSION_FILE_TEXT §6.2 step 1).
//!
//! `pdf-extract` and `lopdf` 0.42 inflate streams with no limit. So while
//! loading, [`cap_object`] (a `lopdf` load filter) empties:
//!
//! - image XObjects: text never needs them, and `pdf-extract` would
//!   inflate one on `Do` and try to read it as page content;
//! - any stream whose `FlateDecode` output, counted through a capped
//!   decoder, passes `MAX_PDF_STREAM_BYTES`.
//!
//! The loader skips the filter for some streams: those whose `/Length`
//! it could only resolve after loading, and every stream of an encrypted
//! file. [`cap_unchecked`] runs the same check over those afterwards.
//!
//! Other filters (LZW, run length) aren't counted. Their bombs, and the
//! object streams of encrypted files, which the loader inflates itself,
//! stay with the worker's allocator cap.
//!
//! [`decoded`] is the capped way for the rest of this module to read a
//! stream.

use kage_core::file_text_wire::MAX_PDF_STREAM_BYTES;
use pdf_extract::{Document, Object, ObjectId, Stream};
use std::io::Read;

const CAP: usize = MAX_PDF_STREAM_BYTES as usize;
const CHUNK: usize = 64 * 1024;

/// The `lopdf` load filter. Caps a stream in place. The loader keeps a
/// file-level object in place and ignores the returned copy; objects from
/// object streams (which are never streams) take the returned value.
pub fn cap_object(id: ObjectId, object: &mut Object) -> Option<(ObjectId, Object)> {
    match object {
        Object::Stream(stream) => {
            cap_stream(stream);
            Some((id, Object::Null))
        }
        other => Some((id, other.clone())),
    }
}

/// Caps the streams the load filter didn't see: all of them for a file
/// that was encrypted, else those read after loading.
pub fn cap_unchecked(doc: &mut Document, all: bool) {
    for object in doc.objects.values_mut() {
        if let Object::Stream(stream) = object {
            if all || stream.start_position.is_some() {
                cap_stream(stream);
            }
        }
    }
}

/// Empties an image or an over-cap stream. Returns whether it did.
pub fn cap_stream(stream: &mut Stream) -> bool {
    let over = is_image(stream) || inflates_past(stream, CAP);
    if over {
        empty(stream);
    }
    over
}

/// Replaces a stream's data, dropping its filters.
pub fn replace(stream: &mut Stream, content: Vec<u8>) {
    stream.set_plain_content(content);
}

fn empty(stream: &mut Stream) {
    replace(stream, Vec::new());
}

fn is_image(stream: &Stream) -> bool {
    stream
        .dict
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|n| n == b"Image")
}

fn filters(stream: &Stream) -> Vec<&[u8]> {
    stream.filters().unwrap_or_default()
}

/// Whether the stream's decoded size passes `cap`. Only `FlateDecode`
/// stages can, and only they are counted; ASCII85 and ASCIIHex stages in
/// front of one are decoded (with the same cap) to reach it.
pub fn inflates_past(stream: &Stream, cap: usize) -> bool {
    let filters = filters(stream);
    let Some(last_flate) = filters.iter().rposition(|f| *f == b"FlateDecode") else {
        return false;
    };
    let mut data = std::borrow::Cow::Borrowed(stream.content.as_slice());
    for (i, filter) in filters.iter().enumerate().take(last_flate + 1) {
        if i == last_flate {
            return inflated_len(&data, cap) > cap;
        }
        let next = match *filter {
            b"FlateDecode" => inflate(&data, cap),
            b"ASCII85Decode" | b"A85" => Some(ascii85(&data)),
            b"ASCIIHexDecode" | b"AHx" => Some(ascii_hex(&data)),
            // Can't count through this one.
            _ => return false,
        };
        match next {
            Some(next) if next.len() <= cap => data = next.into(),
            _ => return true,
        }
    }
    false
}

/// The stream's data with its filters applied, at most `cap` bytes.
/// `None` past the cap. Undecodable data comes back as stored, as
/// `pdf-extract` would use it.
pub fn decoded(stream: &Stream, cap: usize) -> Option<Vec<u8>> {
    if filters(stream).is_empty() {
        return (stream.content.len() <= cap).then(|| stream.content.clone());
    }
    if inflates_past(stream, cap) {
        return None;
    }
    let data = super::caught(|| stream.decompressed_content())
        .and_then(Result::ok)
        .unwrap_or_else(|| stream.content.clone());
    (data.len() <= cap).then_some(data)
}

/// Bytes `FlateDecode` makes of `input`, counting at most `cap + 1`. Like
/// `lopdf`: zlib, and raw deflate after the two-byte header if zlib
/// gives nothing.
fn inflated_len(input: &[u8], cap: usize) -> usize {
    if input.is_empty() {
        return 0;
    }
    let zlib = count(flate2::read::ZlibDecoder::new(input), cap);
    match zlib {
        (n, false) if n == 0 && input.len() > 2 => {
            count(flate2::read::DeflateDecoder::new(&input[2..]), cap).0
        }
        (n, _) => n,
    }
}

/// Reads up to `cap + 1` bytes; returns how many and whether the stream
/// ended cleanly.
fn count(mut reader: impl Read, cap: usize) -> (usize, bool) {
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0usize;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return (total, true),
            Ok(n) => {
                total += n;
                if total > cap {
                    return (total, true);
                }
            }
            Err(_) => return (total, false),
        }
    }
}

fn inflate(input: &[u8], cap: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let limit = cap as u64 + 1;
    let zlib = flate2::read::ZlibDecoder::new(input)
        .take(limit)
        .read_to_end(&mut out);
    if zlib.is_err() && out.is_empty() && input.len() > 2 {
        let _ = flate2::read::DeflateDecoder::new(&input[2..])
            .take(limit)
            .read_to_end(&mut out);
    }
    (out.len() <= cap).then_some(out)
}

fn ascii85(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let (mut group, mut n) = (0u64, 0usize);
    for &b in input {
        match b {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group = group * 85 + u64::from(b - b'!');
                n += 1;
                if n == 5 {
                    out.extend_from_slice(&(group as u32).to_be_bytes());
                    (group, n) = (0, 0);
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        for _ in n..5 {
            group = group * 85 + 84;
        }
        out.extend_from_slice(&(group as u32).to_be_bytes()[..n - 1]);
    }
    out
}

fn ascii_hex(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() / 2);
    let mut high: Option<u8> = None;
    for &b in input {
        if b == b'>' {
            break;
        }
        let Some(v) = (b as char).to_digit(16) else {
            continue;
        };
        match high.take() {
            Some(h) => out.push(h << 4 | v as u8),
            None => high = Some(v as u8),
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    out
}

#[cfg(test)]
#[path = "streams_tests.rs"]
mod tests;

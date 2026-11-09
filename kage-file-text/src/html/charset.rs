//! The encoding an HTML file declares in a `<meta>` tag
//! (EXTENSION_FILE_TEXT §5.9).
//!
//! The WHATWG "prescan a byte stream to determine its encoding"
//! algorithm, over the first 1,024 bytes: comments and other markup are
//! stepped over, and the first `<meta charset=…>` or
//! `<meta http-equiv="content-type" content="…; charset=…">` with a label
//! `encoding_rs` knows wins. A declared UTF-16 (or a label that maps to
//! the "replacement" encoding) means UTF-8, because the bytes were just
//! read as ASCII to find the tag; `x-user-defined` means Windows-1252.
//! Both as in browsers.

use encoding_rs::{Encoding, WINDOWS_1252, X_USER_DEFINED};

/// The bytes the prescan looks at.
pub const PRESCAN_BYTES: usize = 1024;

fn is_space(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

/// The encoding the first 1,024 bytes declare, if any.
pub fn prescan(bytes: &[u8]) -> Option<&'static Encoding> {
    let mut s = Scan {
        b: &bytes[..bytes.len().min(PRESCAN_BYTES)],
        at: 0,
    };
    while s.at < s.b.len() {
        let rest = &s.b[s.at..];
        if rest.starts_with(b"<!--") {
            // The `-->` may share its dashes with the `<!--`.
            let end = memchr::memmem::find(&rest[2..], b"-->")?;
            s.at += 2 + end + 3;
        } else if starts_with_ignore_case(rest, b"<meta")
            && rest.get(5).is_some_and(|&b| is_space(b) || b == b'/')
        {
            s.at += 5;
            if let Some(encoding) = s.meta()? {
                return Some(encoding);
            }
        } else if rest.len() >= 2
            && rest[0] == b'<'
            && (rest[1].is_ascii_alphabetic()
                || (rest[1] == b'/' && rest.get(2).is_some_and(u8::is_ascii_alphabetic)))
        {
            // Another tag: step over its name and attributes.
            let end = rest.iter().position(|&b| is_space(b) || b == b'>')?;
            s.at += end;
            while s.attribute()?.is_some() {}
        } else if rest.starts_with(b"<!") || rest.starts_with(b"</") || rest.starts_with(b"<?") {
            let end = memchr::memchr(b'>', &rest[2..])?;
            s.at += 2 + end + 1;
        } else {
            s.at += 1;
        }
    }
    None
}

fn starts_with_ignore_case(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// A cursor over the prescan window. `None` from a step means the window
/// ended inside it, which ends the prescan.
struct Scan<'a> {
    b: &'a [u8],
    at: usize,
}

impl Scan<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.at).copied()
    }

    fn skip_spaces(&mut self) -> Option<u8> {
        while is_space(self.peek()?) {
            self.at += 1;
        }
        self.peek()
    }

    /// The attributes of a `<meta` tag. `Some(None)`: the tag declares no
    /// usable encoding, so the prescan goes on.
    fn meta(&mut self) -> Option<Option<&'static Encoding>> {
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let (mut got_pragma, mut need_pragma, mut charset) = (false, None, None);
        while let Some((name, value)) = self.attribute()? {
            if seen.contains(&name) {
                continue;
            }
            match name.as_slice() {
                b"http-equiv" if value == b"content-type" => got_pragma = true,
                b"content" if charset.is_none() => {
                    if let Some(label) = charset_in_content(&value) {
                        charset = Encoding::for_label(label);
                        need_pragma = Some(true);
                    }
                }
                b"charset" => {
                    charset = Encoding::for_label(&value);
                    need_pragma = Some(false);
                }
                _ => {}
            }
            seen.push(name);
        }
        let usable = match need_pragma {
            None => false,
            Some(true) => got_pragma,
            Some(false) => true,
        };
        Some(charset.filter(|_| usable).map(|encoding| {
            if encoding == X_USER_DEFINED {
                WINDOWS_1252
            } else {
                encoding.output_encoding()
            }
        }))
    }

    /// One attribute, lowercased. `Some(None)` at the end of the tag.
    fn attribute(&mut self) -> Option<Option<(Vec<u8>, Vec<u8>)>> {
        while {
            let b = self.peek()?;
            is_space(b) || b == b'/'
        } {
            self.at += 1;
        }
        if self.peek()? == b'>' {
            return Some(None);
        }
        let mut name = Vec::new();
        loop {
            let b = self.peek()?;
            if b == b'=' && !name.is_empty() {
                self.at += 1;
                break;
            }
            if is_space(b) {
                if self.skip_spaces()? != b'=' {
                    return Some(Some((name, Vec::new())));
                }
                self.at += 1;
                break;
            }
            if b == b'/' || b == b'>' {
                return Some(Some((name, Vec::new())));
            }
            name.push(b.to_ascii_lowercase());
            self.at += 1;
        }
        let mut value = Vec::new();
        let first = self.skip_spaces()?;
        if first == b'"' || first == b'\'' {
            self.at += 1;
            loop {
                let b = self.peek()?;
                self.at += 1;
                if b == first {
                    return Some(Some((name, value)));
                }
                value.push(b.to_ascii_lowercase());
            }
        }
        if first == b'>' {
            return Some(Some((name, value)));
        }
        loop {
            let b = self.peek()?;
            if is_space(b) || b == b'>' {
                return Some(Some((name, value)));
            }
            value.push(b.to_ascii_lowercase());
            self.at += 1;
        }
    }
}

/// The label after `charset=` in a `content` value (already lowercase).
fn charset_in_content(content: &[u8]) -> Option<&[u8]> {
    let mut at = 0;
    loop {
        at += memchr::memmem::find(&content[at..], b"charset")? + b"charset".len();
        let mut i = at;
        while content.get(i).is_some_and(|&b| is_space(b)) {
            i += 1;
        }
        if content.get(i) != Some(&b'=') {
            continue;
        }
        i += 1;
        while content.get(i).is_some_and(|&b| is_space(b)) {
            i += 1;
        }
        let rest = content.get(i..).filter(|r| !r.is_empty())?;
        return match rest[0] {
            q @ (b'"' | b'\'') => {
                let end = memchr::memchr(q, &rest[1..])?;
                Some(&rest[1..1 + end])
            }
            _ => {
                let end = rest
                    .iter()
                    .position(|&b| is_space(b) || b == b';')
                    .unwrap_or(rest.len());
                Some(&rest[..end])
            }
        };
    }
}

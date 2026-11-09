//! Text normalisation and truncation (EXTENSION_FILE_TEXT §5.10).
//!
//! Every parser feeds its raw text into a [`Normalizer`], which applies,
//! in one pass:
//!
//! 1. Unicode NFC, per `push` (push whole pages or paragraphs, not
//!    fragments of a word, so combining marks meet their base).
//! 2. `\r\n`, `\r`, U+0085, U+2028 and U+2029 become `\n`.
//! 3. Removed: C0 controls except tab and `\n`, DEL, C1 controls, U+FEFF,
//!    noncharacters, tag characters (U+E0000–U+E007F) and the bidi
//!    overrides LRO and RLO. Embeddings, isolates and LRM/RLM stay.
//! 4. Trailing whitespace on each line is dropped, three or more line
//!    breaks become two, and the text has no blank lines at its start or
//!    whitespace at its end.
//! 5. At most `max_chars` Unicode scalar values are kept. Past that the
//!    text is cut at the last paragraph break in the final 5 %, else the
//!    last sentence end there, else the last whitespace there, else
//!    exactly at `max_chars`.
//!
//! [`clean_line`] does the same for names and titles, plus: every bidi
//! control removed, whitespace collapsed to single spaces, one line.

use icu_normalizer::ComposingNormalizerBorrowed;
use std::borrow::Cow;

fn nfc(raw: &str) -> Cow<'_, str> {
    ComposingNormalizerBorrowed::new_nfc().normalize(raw)
}

fn is_noncharacter(u: u32) -> bool {
    (0xFDD0..=0xFDEF).contains(&u) || (u & 0xFFFE) == 0xFFFE
}

/// Characters dropped from document text (step 3).
pub fn is_removed(c: char) -> bool {
    let u = c as u32;
    match u {
        0x09 | 0x0A => false,
        0x00..=0x1F | 0x7F..=0x9F => true,
        0xFEFF | 0x202D | 0x202E => true,
        0xE0000..=0xE007F => true,
        _ => is_noncharacter(u),
    }
}

/// Every bidi control: dropped from names and titles.
pub fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// The result of a [`Normalizer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    pub text: String,
    /// Unicode scalar values in `text`.
    pub chars: u32,
    /// There was more text than `max_chars`.
    pub truncated: bool,
    /// The full normalised length, when it is known.
    pub total_chars: Option<u32>,
    /// Where each page started in `text`, if the parser marked pages.
    /// Pages cut off by truncation are left out.
    pub page_starts: Option<Vec<u32>>,
}

/// Streams raw text into normalised, capped text.
pub struct Normalizer {
    max_chars: usize,
    count_rest: bool,
    text: String,
    chars: usize,
    /// Characters produced, also past the cap while `count_rest` is set.
    produced: u64,
    overflow: bool,
    page_starts: Vec<u64>,
    pages: usize,
    pending_newlines: u8,
    pending_ws: String,
    after_cr: bool,
}

impl Normalizer {
    /// A normalizer that keeps at most `max_chars` characters (at least
    /// one) and stops taking input once it has more.
    pub fn new(max_chars: u32) -> Self {
        let max_chars = max_chars.max(1) as usize;
        Self {
            max_chars,
            count_rest: false,
            text: String::with_capacity(max_chars.min(1 << 16)),
            chars: 0,
            produced: 0,
            overflow: false,
            page_starts: Vec::new(),
            pages: 0,
            pending_newlines: 0,
            pending_ws: String::new(),
            after_cr: false,
        }
    }

    /// Keep counting past the cap, so `finish(true)` can report the full
    /// length. Costs a pass over the rest of the input.
    pub fn counting_rest(mut self) -> Self {
        self.count_rest = true;
        self
    }

    /// There was more text than `max_chars`. A parser can stop.
    pub fn is_full(&self) -> bool {
        self.overflow
    }

    /// Characters kept so far.
    pub fn chars(&self) -> usize {
        self.chars
    }

    fn stopped(&self) -> bool {
        self.overflow && !self.count_rest
    }

    /// Adds raw text. NFC is applied to this piece on its own.
    pub fn push(&mut self, raw: &str) {
        if self.stopped() {
            return;
        }
        for c in nfc(raw).chars() {
            self.feed(c);
            if self.stopped() {
                return;
            }
        }
    }

    /// Ends the line.
    pub fn line_break(&mut self) {
        self.feed('\n');
    }

    /// Ends the paragraph: a blank line follows, unless the text is empty.
    pub fn paragraph_break(&mut self) {
        self.after_cr = false;
        self.pending_ws.clear();
        self.pending_newlines = 2;
    }

    /// Starts a page (or slide): a paragraph break unless it is the first,
    /// and its start offset is recorded for `page_starts`.
    pub fn start_page(&mut self) {
        if self.pages > 0 {
            self.paragraph_break();
        }
        self.pages += 1;
        let ahead = if self.produced > 0 {
            u64::from(self.pending_newlines)
        } else {
            0
        };
        self.page_starts.push(self.produced + ahead);
    }

    fn feed(&mut self, c: char) {
        if self.after_cr {
            self.after_cr = false;
            if c == '\n' {
                return;
            }
        }
        if is_line_break(c) {
            self.after_cr = c == '\r';
            self.pending_ws.clear();
            self.pending_newlines = (self.pending_newlines + 1).min(2);
            return;
        }
        if is_removed(c) {
            return;
        }
        if c.is_whitespace() {
            self.pending_ws.push(c);
            return;
        }
        if self.pending_newlines > 0 {
            if self.produced > 0 {
                for _ in 0..self.pending_newlines {
                    self.emit('\n');
                }
            }
            self.pending_newlines = 0;
        }
        if !self.pending_ws.is_empty() {
            let ws = std::mem::take(&mut self.pending_ws);
            for w in ws.chars() {
                self.emit(w);
            }
            self.pending_ws = ws;
            self.pending_ws.clear();
        }
        self.emit(c);
    }

    fn emit(&mut self, c: char) {
        self.produced += 1;
        if self.chars < self.max_chars {
            self.text.push(c);
            self.chars += 1;
        } else {
            self.overflow = true;
        }
    }

    /// The text. `complete` says the parser pushed the whole document, so
    /// the full length is known (if it was counted).
    pub fn finish(self, complete: bool) -> Normalized {
        let truncated = self.overflow;
        let mut text = self.text;
        if truncated {
            let cut = cut_point(&text, self.max_chars);
            text.truncate(cut);
            let end = text.trim_end().len();
            text.truncate(end);
        }
        let chars = text.chars().count();
        let total_known = complete && (!truncated || self.count_rest);
        let total_chars = total_known.then(|| saturate(self.produced.max(chars as u64)));
        let page_starts = (self.pages > 0).then(|| {
            self.page_starts
                .iter()
                .filter(|&&s| !truncated || s < chars as u64 || s == 0)
                .map(|&s| saturate(s.min(chars as u64)))
                .collect()
        });
        Normalized {
            text,
            chars: saturate(chars as u64),
            truncated,
            total_chars,
            page_starts,
        }
    }
}

fn saturate(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Normalises a whole string with no cap on its length.
pub fn normalize(raw: &str) -> String {
    let mut n = Normalizer::new(u32::MAX);
    n.push(raw);
    n.finish(true).text
}

fn ends_sentence(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…' | '؟' | '।')
}

fn ends_sentence_alone(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '｡')
}

/// Byte offset to cut `text` (exactly `max_chars` chars) at.
fn cut_point(text: &str, max_chars: usize) -> usize {
    let window = max_chars / 20;
    if window == 0 {
        return text.len();
    }
    let count = text.chars().count();
    let floor = text
        .char_indices()
        .nth(count.saturating_sub(window))
        .map_or(text.len(), |(i, _)| i);
    let tail = &text[floor..];
    if let Some(p) = tail.rfind("\n\n") {
        if floor + p > 0 {
            return floor + p;
        }
    }
    let mut sentence = None;
    let mut chars = tail.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next_is_space = chars.peek().is_some_and(|&(_, n)| n.is_whitespace());
        if ends_sentence_alone(c) || (ends_sentence(c) && next_is_space) {
            sentence = Some(i + c.len_utf8());
        }
    }
    if let Some(end) = sentence {
        return floor + end;
    }
    if let Some((p, _)) = tail.char_indices().rev().find(|&(_, c)| c.is_whitespace()) {
        if floor + p > 0 {
            return floor + p;
        }
    }
    text.len()
}

/// A name or title on one clean line of at most `max_chars` characters.
pub fn clean_line(raw: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut n = 0usize;
    let mut space = false;
    for c in nfc(raw).chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if is_removed(c) || is_bidi_control(c) {
            continue;
        }
        let need = if space && n > 0 { 2 } else { 1 };
        if n + need > max_chars {
            break;
        }
        if need == 2 {
            out.push(' ');
        }
        out.push(c);
        n += need;
        space = false;
    }
    out
}

#[cfg(test)]
#[path = "normalize_tests.rs"]
mod tests;

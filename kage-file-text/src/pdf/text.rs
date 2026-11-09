//! A page's text on its way from `pdf-extract` to the normaliser
//! (EXTENSION_FILE_TEXT §6.2 steps 4 and 6).
//!
//! [`PageText`] is the writer `PlainTextOutput` writes into. It refuses
//! text past a budget, so a page of millions of characters stops early.
//! The budget counts only characters the normaliser keeps (not white
//! space or removed controls), at three per character still wanted:
//! NFC can compose at most three into one, so a refused page always
//! holds more text than `max_chars`.
//!
//! [`fix_page`] then repairs what PDF text gets wrong, before §5.10:
//! - Spaces at the start and end of each line go: they come from glyph
//!   positions, not from the document.
//! - A PDF draws right-to-left text in visual order. On each line with
//!   right-to-left letters, the runs are put back in logical order with
//!   the levels of the Unicode bidi algorithm, and brackets inside
//!   right-to-left runs are mirrored back. The paragraph direction of a
//!   line that mixes both is the page's: right to left if the page has
//!   more right-to-left letters than left-to-right ones.
//! - Then Latin ligatures (U+FB00–U+FB06), Arabic presentation forms
//!   (U+FB50–U+FDFF, U+FE70–U+FEFF) and Kangxi radicals (U+2F00–U+2FDF)
//!   get NFKC. Chrome writes `ﬁ` and `⾳`; NFC keeps them, so a search for
//!   "file" or `音` would miss them. After the reordering, so a lam-alef
//!   ligature becomes lam then alef, not the reverse.

use crate::normalize::is_removed;
use icu_normalizer::ComposingNormalizerBorrowed;
use std::fmt;
use unicode_bidi::{bidi_class, BidiClass, Level, ParagraphBidiInfo};

/// Raw page text kept at most, white space included.
const MAX_RAW_BYTES: usize = 64 << 20;

/// The writer for one page.
pub struct PageText {
    text: String,
    budget: usize,
    kept: usize,
    /// Refused because the page had more text than wanted.
    pub full: bool,
    /// Refused because of `MAX_RAW_BYTES`.
    pub overflowed: bool,
}

impl PageText {
    /// A writer for a page when `wanted` more characters are wanted.
    pub fn new(wanted: usize) -> Self {
        Self {
            text: String::new(),
            budget: wanted.saturating_mul(3).saturating_add(16),
            kept: 0,
            full: false,
            overflowed: false,
        }
    }

    pub fn refused(&self) -> bool {
        self.full || self.overflowed
    }

    pub fn into_text(self) -> String {
        self.text
    }
}

impl fmt::Write for PageText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.refused() {
            return Err(fmt::Error);
        }
        for c in s.chars() {
            let counts = !c.is_whitespace() && !is_removed(c);
            if counts && self.kept >= self.budget {
                self.full = true;
                return Err(fmt::Error);
            }
            if self.text.len() + c.len_utf8() > MAX_RAW_BYTES {
                self.overflowed = true;
                return Err(fmt::Error);
            }
            self.kept += usize::from(counts);
            self.text.push(c);
        }
        Ok(())
    }
}

impl<'a> pdf_extract::ConvertToFmt for &'a mut PageText {
    type Writer = &'a mut PageText;
    fn convert(self) -> Self::Writer {
        self
    }
}

fn wants_nfkc(c: char) -> bool {
    matches!(
        c,
        '\u{FB00}'..='\u{FB06}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}' | '\u{2F00}'..='\u{2FDF}'
    )
}

fn is_rtl(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

fn is_ltr(c: char) -> bool {
    bidi_class(c) == BidiClass::L
}

/// The bracket a mirrored glyph stands for.
fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        '﴾' => '﴿',
        '﴿' => '﴾',
        other => other,
    }
}

/// A page's text with the fixes in the module docs.
pub fn fix_page(raw: &str) -> String {
    let (mut rtl, mut ltr) = (0usize, 0usize);
    let mut nfkc_needed = false;
    for c in raw.chars() {
        rtl += usize::from(is_rtl(c));
        ltr += usize::from(is_ltr(c));
        nfkc_needed |= wants_nfkc(c);
    }
    let page_level = if rtl > ltr {
        Level::rtl()
    } else {
        Level::ltr()
    };
    let nfkc = ComposingNormalizerBorrowed::new_nfkc();
    let mut out = String::with_capacity(raw.len());
    for (i, line) in raw.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line = line.trim_matches(' ');
        let ordered = if rtl > 0 && line.chars().any(is_rtl) {
            let level = if line.chars().any(is_ltr) {
                page_level
            } else {
                Level::rtl()
            };
            std::borrow::Cow::Owned(logical_order(line, level))
        } else {
            std::borrow::Cow::Borrowed(line)
        };
        if !nfkc_needed {
            out.push_str(&ordered);
            continue;
        }
        for c in ordered.chars() {
            if wants_nfkc(c) {
                let mut one = [0u8; 4];
                out.push_str(&nfkc.normalize(c.encode_utf8(&mut one)));
            } else {
                out.push(c);
            }
        }
    }
    out
}

/// A visually ordered line in logical order, for a paragraph `level`.
fn logical_order(line: &str, level: Level) -> String {
    let info = ParagraphBidiInfo::new(line, Some(level));
    let (levels, runs) = info.visual_runs(0..line.len());
    let mut out = String::with_capacity(line.len());
    for run in runs {
        let text = &line[run.clone()];
        if levels[run.start].is_rtl() {
            out.extend(text.chars().rev().map(mirror));
        } else {
            out.push_str(text);
        }
    }
    out
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;

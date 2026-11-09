//! Visible HTML text into a [`Normalizer`].
//!
//! Outside preformatted elements, runs of HTML whitespace (space, tab,
//! line feed, form feed, carriage return) become one space, and none is
//! kept at the start of a line, as a browser lays text out. Other spaces
//! (no-break, ideographic) stay. Breaks come from the elements
//! (`rules::break_of`).
//!
//! Text is collected per block and pushed to the normalizer at each
//! break, so NFC sees whole paragraphs even when the tokenizer split them
//! (around a character reference, say). A long block is pushed in pieces,
//! each cut just before an ASCII character, which never composes with
//! what comes before it.

use crate::normalize::{Normalized, Normalizer};

/// A block longer than this is pushed in pieces.
const FLUSH_BYTES: usize = 64 * 1024;

fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

pub struct Writer {
    norm: Normalizer,
    block: String,
    /// Line breaks due before the next text: 1 a line, 2 a paragraph.
    /// Block edges merge (`</div><div>` is one break); `<br>`s add up.
    breaks: u8,
    /// A collapsed space is due before the next character.
    space: bool,
    /// Whitespace here is dropped: the start of a line or a table cell.
    fresh: bool,
    /// A heading or list prefix, written before the block's first
    /// character.
    prefix: Option<&'static str>,
}

impl Writer {
    pub fn new(max_chars: u32) -> Self {
        Self {
            norm: Normalizer::new(max_chars).counting_rest(),
            block: String::new(),
            breaks: 0,
            space: false,
            fresh: true,
            prefix: None,
        }
    }

    fn put(&mut self, c: char) {
        if c.is_ascii() && self.block.len() >= FLUSH_BYTES {
            self.flush();
        }
        self.block.push(c);
    }

    fn flush(&mut self) {
        if !self.block.is_empty() {
            self.norm.push(&self.block);
            self.block.clear();
        }
    }

    /// Writes the breaks due, before new text.
    fn apply_breaks(&mut self) {
        match std::mem::take(&mut self.breaks) {
            0 => {}
            1 => self.norm.line_break(),
            _ => self.norm.paragraph_break(),
        }
    }

    fn start_text(&mut self) {
        self.apply_breaks();
        if let Some(prefix) = self.prefix.take() {
            prefix.chars().for_each(|c| self.put(c));
            self.space = false;
        }
        if std::mem::take(&mut self.space) {
            self.put(' ');
        }
        self.fresh = false;
    }

    /// Text in normal flow: whitespace collapses.
    pub fn text(&mut self, text: &str) {
        for c in text.chars() {
            if is_html_space(c) {
                self.space |= !self.fresh;
                continue;
            }
            self.start_text();
            self.put(c);
        }
    }

    /// Text in a preformatted element: kept as it is.
    pub fn preformatted(&mut self, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                self.apply_breaks();
            } else {
                self.start_text();
            }
            self.put(c);
            self.fresh = c == '\n';
        }
    }

    /// A space between two words that a block boundary would otherwise
    /// glue (blocks inside a table cell stay on the row's line).
    pub fn space(&mut self) {
        self.space |= !self.fresh;
    }

    fn end_block(&mut self, breaks: u8) {
        self.flush();
        self.breaks = breaks.min(2);
        self.space = false;
        self.fresh = true;
    }

    /// The edge of a block on a line of its own.
    pub fn line_break(&mut self) {
        self.end_block(self.breaks.max(1));
    }

    /// The edge of a block set apart by a blank line.
    pub fn paragraph_break(&mut self) {
        self.end_block(2);
    }

    /// `<br>`: one more line break.
    pub fn hard_break(&mut self) {
        self.end_block(self.breaks + 1);
    }

    /// Between two table cells.
    pub fn tab(&mut self) {
        self.apply_breaks();
        self.space = false;
        self.put('\t');
        self.fresh = true;
    }

    /// Sets the prefix of the next block's text; a later one replaces it.
    pub fn set_prefix(&mut self, prefix: &'static str) {
        self.prefix = Some(prefix);
    }

    /// Drops a prefix no text used (an empty list item or heading).
    pub fn clear_prefix(&mut self) {
        self.prefix = None;
    }

    pub fn finish(mut self) -> Normalized {
        self.flush();
        self.norm.finish(true)
    }
}

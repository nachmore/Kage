//! Body text into a [`Normalizer`].
//!
//! Text is collected per paragraph and pushed at each break, so NFC sees
//! a letter and the combining mark a `\uN` added after it together. A
//! long paragraph is pushed in pieces, each cut just before an ASCII
//! character, which never composes with what comes before it.
//!
//! A paragraph's prefix (`# ` for a heading, `- ` for a list item) goes
//! before its first non-blank text. Paragraphs are set apart by a blank
//! line, but the items of one list by a line break, so a list reads as a
//! Markdown list.

use crate::normalize::{Normalized, Normalizer};

/// A paragraph longer than this is pushed in pieces.
const FLUSH_BYTES: usize = 64 * 1024;

pub struct Out {
    norm: Normalizer,
    block: String,
    /// No text yet in this paragraph.
    para_start: bool,
    /// This paragraph is a list item.
    list_item: bool,
    /// A paragraph ended; `true` if it was a list item with text.
    pending: Option<bool>,
}

impl Out {
    pub fn new(max_chars: u32) -> Self {
        Self {
            norm: Normalizer::new(max_chars).counting_rest(),
            block: String::new(),
            para_start: true,
            list_item: false,
            pending: None,
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

    /// Writes the break due before a paragraph; `list_item` says whether
    /// the new one is a list item.
    fn apply_pending(&mut self, list_item: bool) {
        match self.pending.take() {
            Some(true) if list_item => self.norm.line_break(),
            Some(_) => self.norm.paragraph_break(),
            None => {}
        }
    }

    /// Paragraph text. `prefix` applies if this is the paragraph's first.
    pub fn text(&mut self, text: &str, prefix: Option<&str>) {
        let mut text = text;
        if self.para_start {
            if prefix.is_some() {
                text = text.trim_start();
            }
            if text.is_empty() {
                return;
            }
            let list_item = prefix == Some("- ");
            self.apply_pending(list_item);
            self.list_item = list_item;
            prefix.unwrap_or("").chars().for_each(|c| self.put(c));
            self.para_start = false;
        }
        text.chars().for_each(|c| self.put(c));
    }

    /// `\par`, `\sect`, `\page`.
    pub fn paragraph_break(&mut self) {
        self.flush();
        self.pending = Some(!self.para_start && self.list_item);
        self.para_start = true;
        self.list_item = false;
    }

    /// `\line`. With `new_paragraph` (`\row`), what follows can take a
    /// prefix again.
    pub fn line_break(&mut self, new_paragraph: bool) {
        self.flush();
        self.apply_pending(false);
        self.norm.line_break();
        self.para_start |= new_paragraph;
    }

    /// `\cell`.
    pub fn tab(&mut self) {
        self.apply_pending(false);
        self.put('\t');
    }

    pub fn finish(mut self) -> Normalized {
        self.flush();
        self.norm.finish(true)
    }
}

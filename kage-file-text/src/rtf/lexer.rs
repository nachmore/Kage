//! RTF tokens (EXTENSION_FILE_TEXT §5.9).
//!
//! - `{` and `}` open and close groups.
//! - `\word` with an optional signed numeric parameter, then an optional
//!   space that belongs to the word. Only the first 32 letters of a word
//!   are kept and a parameter saturates, so an absurd one just doesn't
//!   match anything.
//! - `\'hh`: one byte, in the current code page.
//! - `\` and another character: a control symbol. `\` before a line
//!   break is `\par`.
//! - Anything else is text, except bare line breaks, which RTF ignores.
//! - `\binN` is followed by N raw bytes, which are skipped here (at most
//!   to the end of the input), so binary data can't look like markup.

/// Longest control word kept, as in the RTF specification.
const MAX_WORD: usize = 32;
/// Digits read into a parameter; more saturate.
const MAX_DIGITS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token<'a> {
    Open,
    Close,
    Word(&'a [u8], Option<i32>),
    Symbol(u8),
    Byte(u8),
    Text(&'a [u8]),
}

pub struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn control(&mut self) -> Option<Token<'a>> {
        let b = self.peek()?;
        if !b.is_ascii_alphabetic() {
            self.at += 1;
            return match b {
                b'\'' => self.hex_byte(),
                b'\r' | b'\n' => Some(Token::Word(b"par", None)),
                _ => Some(Token::Symbol(b)),
            };
        }
        let start = self.at;
        while self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
            self.at += 1;
        }
        let name = &self.bytes[start..self.at.min(start + MAX_WORD)];
        let param = self.parameter();
        if self.peek() == Some(b' ') {
            self.at += 1;
        }
        if name == b"bin" {
            let skip = usize::try_from(param.unwrap_or(0).max(0)).unwrap_or(0);
            self.at = self.at.saturating_add(skip).min(self.bytes.len());
        }
        Some(Token::Word(name, param))
    }

    fn parameter(&mut self) -> Option<i32> {
        let negative = self.peek() == Some(b'-')
            && self.bytes.get(self.at + 1).is_some_and(u8::is_ascii_digit);
        if negative {
            self.at += 1;
        }
        let start = self.at;
        let mut value: i64 = 0;
        while let Some(d) = self.peek().filter(u8::is_ascii_digit) {
            if self.at - start < MAX_DIGITS {
                value = value * 10 + i64::from(d - b'0');
            }
            self.at += 1;
        }
        if self.at == start {
            return None;
        }
        let value = if negative { -value } else { value };
        Some(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
    }

    fn hex_byte(&mut self) -> Option<Token<'a>> {
        let digit = |b: Option<u8>| (b? as char).to_digit(16);
        let high = digit(self.peek());
        let low = digit(self.bytes.get(self.at + 1).copied());
        match (high, low) {
            (Some(h), Some(l)) => {
                self.at += 2;
                Some(Token::Byte((h * 16 + l) as u8))
            }
            // A broken escape: a symbol that means nothing.
            _ => Some(Token::Symbol(b'\'')),
        }
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            let b = self.peek()?;
            self.at += 1;
            return match b {
                b'{' => Some(Token::Open),
                b'}' => Some(Token::Close),
                b'\\' => self.control(),
                b'\r' | b'\n' => continue,
                _ => {
                    let start = self.at - 1;
                    let run = self.bytes[self.at..]
                        .iter()
                        .position(|b| matches!(b, b'{' | b'}' | b'\\' | b'\r' | b'\n'))
                        .unwrap_or(self.bytes.len() - self.at);
                    self.at += run;
                    Some(Token::Text(&self.bytes[start..self.at]))
                }
            };
        }
    }
}

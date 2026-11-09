//! RTF: the lexer (`\binN`, escapes, parameters), the group stack and its
//! depth cap, destinations, code pages and font charsets, `\uN` with its
//! fallback and surrogates, notes, the title and the caps. Uses
//! `extract_as`, so it runs before the P1 formats are switched on.
//!
//! Seeds: `fuzz/seeds/rtf/`, and `fuzz/dict/rtf.dict` as `-dict=`.
#![no_main]

use kage_file_text::Format;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| kage_file_text_fuzz::check(Format::Rtf, data));

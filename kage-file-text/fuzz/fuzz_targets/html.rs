//! HTML: decoding (BOM, `<meta charset>`, the plain text order), the
//! tokenizer sink (raw text, skipped and hidden elements, implied ends,
//! foreign content), layout, the title and the caps. Uses `extract_as`,
//! so it runs before the P1 formats are switched on.
//!
//! Seeds: `fuzz/seeds/html/`, and `fuzz/dict/html.dict` as `-dict=`.
#![no_main]

use kage_file_text::Format;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| kage_file_text_fuzz::check(Format::Html, data));

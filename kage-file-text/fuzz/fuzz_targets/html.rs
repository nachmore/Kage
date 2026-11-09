//! HTML: detection now, the reader once WP-FT4 lands. Uses `extract_as`,
//! so it runs before the P1 formats are switched on.
#![no_main]

use kage_file_text::Format;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| kage_file_text_fuzz::check(Format::Html, data));

//! PDF: detection now, the reader once WP-FT3 lands.
#![no_main]

use kage_file_text::Format;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| kage_file_text_fuzz::check(Format::Pdf, data));

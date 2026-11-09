//! Plain text and Markdown: decoding, the binary check, normalisation,
//! truncation and Markdown titles.
#![no_main]

use kage_file_text::Format;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    kage_file_text_fuzz::check(Format::Text, data);
    kage_file_text_fuzz::check(Format::Markdown, data);
});

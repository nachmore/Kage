//! DOCX and PPTX: package detection now, the readers once WP-FT2 lands.
#![no_main]

use kage_file_text::Format;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    kage_file_text_fuzz::check(Format::Docx, data);
    kage_file_text_fuzz::check(Format::Pptx, data);
});

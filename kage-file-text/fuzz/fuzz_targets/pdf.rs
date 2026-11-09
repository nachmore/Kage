//! PDF: loading, repair, the page pass, fonts, forms and the text fixes.
//!
//! Seeds: `KAGE_FILE_TEXT_SEEDS=fuzz/corpus/pdf cargo test -p
//! kage-file-text --test pdf_seeds`, run in `kage-file-text/`. PDF
//! keywords for the mutator: `cargo fuzz run pdf -s none -- -dict=dicts/pdf.dict`.
//!
//! `pdf-extract` panics on many broken pages, and the reader catches that
//! for each page (EXTENSION_FILE_TEXT §6.2 step 2). The panic hook
//! libfuzzer-sys installs would abort on the first such panic, so this
//! target swaps it for a silent one. A panic that escapes the reader is
//! still a crash: libfuzzer-sys aborts when one reaches it.
#![no_main]

use kage_file_text::Format;
use libfuzzer_sys::fuzz_target;
use std::sync::Once;

static QUIET_PANICS: Once = Once::new();

fuzz_target!(|data: &[u8]| {
    QUIET_PANICS.call_once(|| std::panic::set_hook(Box::new(|_| {})));
    kage_file_text_fuzz::check(Format::Pdf, data);
});

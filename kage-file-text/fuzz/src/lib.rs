//! The invariants every fuzz target checks (EXTENSION_FILE_TEXT §12).
//!
//! The first input byte picks the character cap, so the truncation paths
//! get exercised too; the rest is the file. A panic inside `extract_as`
//! is a finding: the worker would turn it into `malformed`, but a reader
//! should not panic on any input.

use kage_core::file_text_wire::MAX_TITLE_CHARS;
use kage_file_text::normalize::is_removed;
use kage_file_text::{extract_as, Format, Job, WorkerResponse};

/// Caps the first byte chooses from: tiny, odd, the default and the most.
const CAPS: [u32; 8] = [1, 7, 20, 100, 1_000, 4_096, 100_000, 1_000_000];

/// Reads `data` as `format` and checks the result.
pub fn check(format: Format, data: &[u8]) {
    let (cap, bytes) = match data.split_first() {
        Some((first, rest)) => (CAPS[usize::from(*first) % CAPS.len()], rest),
        None => (100, data),
    };
    let response = extract_as(&Job::new(format, bytes, cap));
    let WorkerResponse::Ok {
        format: got,
        text,
        chars,
        total_chars,
        title,
        page_starts,
        ..
    } = response
    else {
        return;
    };
    assert_eq!(got, format);
    assert_eq!(
        text.chars().count(),
        chars as usize,
        "chars counts scalar values"
    );
    assert!(chars <= cap, "{chars} chars over the cap of {cap}");
    if let Some(total) = total_chars {
        assert!(total >= chars);
    }
    assert_normalized(&text);
    if let Some(title) = title {
        assert!(!title.is_empty() && title.chars().count() <= MAX_TITLE_CHARS);
        assert!(!title.contains(char::is_control) && title.trim() == title);
    }
    if let Some(starts) = page_starts {
        assert!(starts.windows(2).all(|w| w[0] <= w[1]));
        assert!(starts.iter().all(|&s| s <= chars));
    }
}

/// §5.10: no removed characters, no `\r`, no runs of three line breaks,
/// no blank start, no trailing whitespace on any line.
fn assert_normalized(text: &str) {
    assert!(!text.chars().any(|c| is_removed(c) || c == '\r'));
    assert!(!text.contains("\n\n\n"));
    assert!(!text.starts_with('\n'));
    for line in text.split('\n') {
        assert_eq!(line.trim_end(), line, "trailing whitespace");
    }
}

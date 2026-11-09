use super::*;

fn norm(raw: &str) -> String {
    normalize(raw)
}

fn capped(raw: &str, max: u32) -> Normalized {
    let mut n = Normalizer::new(max).counting_rest();
    n.push(raw);
    n.finish(true)
}

#[test]
fn nfc_composes() {
    assert_eq!(norm("e\u{301}te\u{301}"), "\u{e9}t\u{e9}");
    // Already composed text is left alone.
    assert_eq!(norm("caf\u{e9}"), "caf\u{e9}");
    // Hangul jamo compose too.
    assert_eq!(norm("\u{1100}\u{1161}"), "\u{ac00}");
}

#[test]
fn line_endings_become_newlines() {
    assert_eq!(norm("a\r\nb\rc\nd"), "a\nb\nc\nd");
    assert_eq!(norm("a\u{2028}b\u{2029}c\u{85}d"), "a\nb\nc\nd");
    // A CR at the end of one push and an LF at the start of the next are
    // one break.
    let mut n = Normalizer::new(100);
    n.push("a\r");
    n.push("\nb");
    assert_eq!(n.finish(true).text, "a\nb");
}

#[test]
fn removes_each_character_class() {
    let cases = [
        ("NUL", "a\u{0}b"),
        ("C0", "a\u{1}\u{8}\u{b}\u{c}\u{1b}\u{1f}b"),
        ("DEL", "a\u{7f}b"),
        ("C1", "a\u{80}\u{9f}b"),
        ("BOM", "\u{feff}a\u{feff}b"),
        (
            "noncharacters",
            "a\u{fdd0}\u{fdef}\u{fffe}\u{ffff}\u{1fffe}\u{10ffff}b",
        ),
        ("tags", "a\u{e0001}\u{e0041}\u{e007f}b"),
        ("LRO and RLO", "a\u{202d}\u{202e}b"),
    ];
    for (what, raw) in cases {
        assert_eq!(norm(raw), "ab", "{what}");
    }
}

#[test]
fn keeps_tabs_embeddings_isolates_and_marks() {
    let kept = "a\tb \u{202a}c\u{202c} \u{202b}d\u{202c} \u{2066}e\u{2069} \u{2067}f\u{2069} \u{2068}g\u{2069} \u{200e}\u{200f}\u{61c}";
    assert_eq!(norm(kept), kept);
}

#[test]
fn trims_trailing_whitespace_and_collapses_blank_lines() {
    assert_eq!(norm("a  \nb\t\n c"), "a\nb\n c");
    assert_eq!(norm("a\n\n\n\n\nb"), "a\n\nb");
    assert_eq!(norm("a\n  \n\t\n \nb"), "a\n\nb");
    assert_eq!(norm("a\u{a0}\u{3000}\nb"), "a\nb");
    // No blank lines at the start, no whitespace at the end.
    assert_eq!(norm("\n\n\n  a\n\n  \n"), "  a");
    assert_eq!(norm(" \n\t\n"), "");
}

#[test]
fn counts_scalar_values() {
    let n = capped("😀 é 漢字", 100);
    assert_eq!(n.chars, 6);
    assert_eq!(n.text.chars().count(), 6);
    assert!(n.text.encode_utf16().count() > 6);
    assert_eq!(n.total_chars, Some(6));
    assert!(!n.truncated);
}

#[test]
fn short_text_isnt_truncated() {
    let n = capped("hello", 5);
    assert_eq!(n.text, "hello");
    assert!(!n.truncated);
    assert_eq!(n.total_chars, Some(5));
}

#[test]
fn hard_cut_without_a_boundary() {
    let n = capped(&"x".repeat(250), 100);
    assert_eq!(n.text, "x".repeat(100));
    assert_eq!(n.chars, 100);
    assert!(n.truncated);
    assert_eq!(n.total_chars, Some(250));
}

#[test]
fn cuts_at_the_last_paragraph_break_in_the_last_5_percent() {
    // 200 chars kept; the window is the last 10.
    let raw = format!("{}\n\n{}", "a".repeat(193), "b".repeat(50));
    let n = capped(&raw, 200);
    assert_eq!(n.text, "a".repeat(193));
    assert!(n.truncated);
    // A paragraph break before the window doesn't count.
    let raw = format!("{}\n\n{}", "a".repeat(150), "b".repeat(100));
    let n = capped(&raw, 200);
    assert_eq!(n.chars, 200);
}

#[test]
fn cuts_after_the_last_sentence_end() {
    let raw = format!("{}. And {}", "a".repeat(193), "b".repeat(50));
    let n = capped(&raw, 200);
    assert_eq!(n.text, format!("{}.", "a".repeat(193)));
    // CJK full stops need no space after them.
    let raw = format!("{}。{}", "字".repeat(194), "字".repeat(50));
    let n = capped(&raw, 200);
    assert_eq!(n.text, format!("{}。", "字".repeat(194)));
    // A paragraph break wins over a later sentence end.
    let raw = format!(
        "{}\n\n{}. {}",
        "a".repeat(191),
        "b".repeat(3),
        "c".repeat(50)
    );
    assert_eq!(capped(&raw, 200).text, "a".repeat(191));
}

#[test]
fn cuts_at_the_last_whitespace() {
    let raw = format!("{} {}", "a".repeat(195), "b".repeat(50));
    let n = capped(&raw, 200);
    assert_eq!(n.text, "a".repeat(195));
    // A full stop inside a word is no sentence end.
    let raw = format!("{} x.y{}", "a".repeat(193), "b".repeat(50));
    assert_eq!(capped(&raw, 200).text, "a".repeat(193));
}

#[test]
fn tiny_caps_cut_hard() {
    let n = capped("hello world, this is long", 7);
    assert_eq!(n.text, "hello w");
    let n = capped("ab", 1);
    assert_eq!(n.text, "a");
    assert!(n.truncated);
}

#[test]
fn a_cut_never_leaves_trailing_whitespace() {
    let raw = format!("{}   {}", "a".repeat(98), "b".repeat(10));
    let n = capped(&raw, 100);
    assert_eq!(n.text, "a".repeat(98));
}

#[test]
fn total_is_unknown_when_not_counted_or_incomplete() {
    let mut n = Normalizer::new(10);
    n.push(&"x".repeat(100));
    assert!(n.is_full());
    let out = n.finish(true);
    assert!(out.truncated);
    assert_eq!(out.total_chars, None);

    let mut n = Normalizer::new(1000);
    n.push("abc");
    let out = n.finish(false);
    assert_eq!(out.total_chars, None);
    assert_eq!(out.chars, 3);
}

#[test]
fn stops_taking_input_once_full() {
    let mut n = Normalizer::new(3);
    n.push("abcd");
    assert!(n.is_full());
    n.push("efgh");
    assert_eq!(n.chars(), 3);
    assert_eq!(n.finish(false).text, "abc");
}

#[test]
fn page_starts_follow_the_text() {
    let mut n = Normalizer::new(1000);
    for page in ["one", "", "  two\n", "three"] {
        n.start_page();
        n.push(page);
    }
    let out = n.finish(true);
    assert_eq!(out.text, "one\n\n  two\n\nthree");
    assert_eq!(out.page_starts, Some(vec![0, 5, 5, 12]));
    // Without pages there are none.
    assert_eq!(capped("x", 10).page_starts, None);
}

#[test]
fn page_starts_drop_pages_past_the_cut() {
    let mut n = Normalizer::new(24);
    for page in ["first page", "second page", "third page"] {
        n.start_page();
        n.push(page);
        if n.is_full() {
            break;
        }
    }
    let out = n.finish(false);
    assert!(out.truncated);
    assert_eq!(out.text, "first page\n\nsecond page");
    // The third page started at 25, past the cut.
    assert_eq!(out.page_starts, Some(vec![0, 12]));
}

#[test]
fn empty_first_pages_start_at_zero() {
    let mut n = Normalizer::new(100);
    n.start_page();
    n.start_page();
    n.push("x");
    n.start_page();
    let out = n.finish(true);
    assert_eq!(out.text, "x");
    assert_eq!(out.page_starts, Some(vec![0, 0, 1]));
}

#[test]
fn breaks_from_the_parser() {
    let mut n = Normalizer::new(100);
    n.push("a");
    n.line_break();
    n.push("b");
    n.paragraph_break();
    n.paragraph_break();
    n.push("c");
    assert_eq!(n.finish(true).text, "a\nb\n\nc");
}

#[test]
fn clean_line_is_one_short_clean_line() {
    assert_eq!(clean_line("  Q3\tplan\r\n  final  ", 100), "Q3 plan final");
    assert_eq!(
        clean_line("re\u{301}sume\u{301}.pdf", 100),
        "r\u{e9}sum\u{e9}.pdf"
    );
    assert_eq!(
        clean_line(
            "a\u{202e}fdp.exe\u{202c}\u{200f}\u{2066}b\u{2069}\u{61c}",
            100
        ),
        "afdp.exeb"
    );
    assert_eq!(clean_line("a\u{0}b\u{feff}c\u{e0041}d", 100), "abcd");
    assert_eq!(clean_line("\u{b}x\u{c}y\u{85}z", 100), "x y z");
    assert_eq!(clean_line(" \n\t ", 100), "");
}

#[test]
fn clean_line_caps_without_a_trailing_space() {
    assert_eq!(clean_line("abcdef", 4), "abcd");
    assert_eq!(clean_line("abc def", 4), "abc");
    assert_eq!(clean_line("abc def", 5), "abc d");
    assert_eq!(clean_line(&"é".repeat(300), 255).chars().count(), 255);
}

use super::*;
use std::fmt::Write;

fn visual(logical: &str) -> String {
    logical.chars().rev().collect()
}

#[test]
fn the_budget_counts_kept_characters_only() {
    let mut page = PageText::new(2);
    // Budget: 2 * 3 + 16 = 22 kept characters; white space is free.
    assert!(write!(page, "{}", " \n\t".repeat(100)).is_ok());
    assert!(write!(page, "{}", "a".repeat(22)).is_ok());
    assert!(!page.refused());
    assert!(write!(page, "b").is_err());
    assert!(page.full && !page.overflowed);
    assert!(write!(page, " ").is_err(), "stays refused");
    assert_eq!(page.into_text().matches('a').count(), 22);
}

#[test]
fn ligatures_and_radicals_are_unfolded() {
    assert_eq!(fix_page("\u{FB01}le \u{FB00}"), "file ff");
    assert_eq!(fix_page("\u{2FB3}"), "音");
    // Nothing else changes: NFC and the rest are §5.10's job.
    assert_eq!(fix_page("Ⅻ ① ﬀ"), "Ⅻ ① ff");
    assert_eq!(fix_page("plain"), "plain");
}

#[test]
fn line_edges_lose_their_spaces() {
    assert_eq!(fix_page("  indented \n  next  "), "indented\nnext");
}

#[test]
fn arabic_presentation_forms_become_letters() {
    // مرحبا in presentation forms, drawn left to right.
    let drawn = "\u{FE8E}\u{FE92}\u{FEA3}\u{FEAE}\u{FEE3}";
    assert_eq!(fix_page(drawn), "مرحبا");
}

#[test]
fn a_lam_alef_ligature_unfolds_in_logical_order() {
    // سلام with the lam-alef ligature, drawn right to left.
    let drawn = visual("\u{FEB3}\u{FEFC}\u{FEE1}");
    assert_eq!(fix_page(&drawn), "سلام");
}

#[test]
fn hebrew_lines_go_back_to_logical_order() {
    assert_eq!(fix_page(&visual("שלום עולם")), "שלום עולם");
}

#[test]
fn embedded_left_to_right_words_and_numbers_survive() {
    // Logical "שלום world 123 עולם" in an RTL paragraph, as drawn:
    // the Hebrew words reversed, the run order reversed, the
    // left-to-right run as it is.
    let drawn = "םלוע world 123 םולש";
    assert_eq!(fix_page(drawn), "שלום world 123 עולם");
    assert_eq!(fix_page("123 םולש"), "שלום 123");
}

#[test]
fn a_hebrew_word_in_an_english_line() {
    assert_eq!(fix_page("Say םולש to them"), "Say שלום to them");
}

/// How a renderer draws `logical` in a paragraph of `level`, left to
/// right, with a mirrored bracket recorded as the bracket it looks like.
fn drawn(logical: &str, level: Level) -> String {
    let info = ParagraphBidiInfo::new(logical, Some(level));
    let (levels, runs) = info.visual_runs(0..logical.len());
    let mut out = String::new();
    for run in runs {
        let text = &logical[run.clone()];
        if levels[run.start].is_rtl() {
            out.extend(text.chars().rev().map(mirror));
        } else {
            out.push_str(text);
        }
    }
    out
}

#[test]
fn drawn_lines_come_back() {
    let rtl = [
        "שלום עולם",
        "שלום world 123 עולם",
        "הסכם רישיון תוכנה עבור macOS Tahoe 26",
        "יש לקרוא את הסכם רישיון תוכנה זה (\"הרישיון\") בקפידה",
        "شركة APPLE INC.",
        "اتفاقية ترخيص البرامج (\"الترخيص\")",
    ];
    // Each on a right-to-left page.
    let more = "הסכם רישיון תוכנה ארוך מאוד בשפה העברית";
    for logical in rtl {
        let page = format!(
            "{}\n{}",
            drawn(logical, Level::rtl()),
            drawn(more, Level::rtl())
        );
        assert_eq!(fix_page(&page), format!("{logical}\n{more}"));
    }
    // Lines of an English page with a right-to-left word inside.
    let page = [
        "Say שלום to them",
        "The word (שלום) in brackets",
        "Plenty of English text keeps this page left to right.",
    ];
    let drawn_page: Vec<String> = page.iter().map(|l| drawn(l, Level::ltr())).collect();
    assert_eq!(fix_page(&drawn_page.join("\n")), page.join("\n"));
}

#[test]
fn a_mixed_line_takes_the_pages_direction() {
    // The same drawn line reads right to left on an Arabic page and left
    // to right on an English one.
    let line = drawn("شركة APPLE", Level::rtl());
    let arabic = format!(
        "{line}\n{}",
        drawn("هذه اتفاقية ترخيص البرامج", Level::rtl())
    );
    assert!(fix_page(&arabic).starts_with("شركة APPLE\n"));
    let english = format!("{line}\nAnd a long English line that sets the direction.");
    assert!(fix_page(&english).starts_with("APPLE شركة\n"));
}

#[test]
fn brackets_in_right_to_left_runs_are_mirrored_back() {
    // Logical ("הרישיון"), drawn with each bracket's shape as its character.
    assert_eq!(fix_page("(\"ןוישירה\")"), "(\"הרישיון\")");
    // Brackets in left-to-right text are left alone.
    assert_eq!(
        fix_page("(a) said םולש in the text"),
        "(a) said שלום in the text"
    );
}

#[test]
fn only_lines_with_rtl_letters_change() {
    let page = "first line\nםולש\nlast line";
    assert_eq!(fix_page(page), "first line\nשלום\nlast line");
}

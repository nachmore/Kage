use super::*;

fn calls(content: &str) -> Vec<(String, u64)> {
    let mut out: Vec<_> = super::content(content.as_bytes(), false)
        .calls
        .into_iter()
        .map(|(k, v)| (String::from_utf8(k).unwrap(), v))
        .collect();
    out.sort();
    out
}

fn shown(content: &[u8]) -> Vec<(String, Vec<u16>)> {
    let mut out: Vec<_> = super::content(content, true)
        .shown
        .into_iter()
        .map(|(k, v)| (String::from_utf8(k).unwrap(), v.into_iter().collect()))
        .collect();
    out.sort();
    out
}

#[test]
fn finds_do_calls_with_counts() {
    let content = "q 1 0 0 1 0 0 cm /Fm0 Do Q /Fm0 Do\n/Im#201 Do BT (/Fake Do) Tj ET";
    assert_eq!(calls(content), vec![("Fm0".into(), 2), ("Im 1".into(), 1)]);
}

#[test]
fn ignores_strings_comments_and_dictionaries() {
    let content = "% /A Do\n<</B 1>> BDC (x\\) /C Do) Tj <2F44> Tj EMC /D Do";
    assert_eq!(calls(content), vec![("D".into(), 1)]);
}

#[test]
fn a_name_must_be_the_operand_of_do() {
    assert!(calls("Do /A").is_empty());
    assert!(calls("/A cm Do").is_empty());
    // Like `pdf-extract`, the first operand counts.
    assert_eq!(calls("/A /B Do"), vec![("A".into(), 1)]);
}

#[test]
fn skips_inline_image_data() {
    let content = "BI /W 1 /H 1 /BPC 8 /CS /G ID \x00/X Do\x01 EI /Y Do";
    assert_eq!(calls(content), vec![("Y".into(), 1)]);
}

#[test]
fn records_codes_per_font() {
    let content = b"BT /F1 12 Tf <0CD4 0E8A> Tj [<034B> -120 <0001>] TJ /F2 9 Tf (AB) Tj ET";
    assert_eq!(
        shown(content),
        vec![
            ("F1".into(), vec![0x0001, 0x034B, 0x0CD4, 0x0E8A]),
            ("F2".into(), vec![0x4142]),
        ]
    );
}

#[test]
fn q_and_q_restore_the_font() {
    let content = b"/F1 12 Tf q /F2 12 Tf <0002> Tj Q <0001> Tj";
    assert_eq!(
        shown(content),
        vec![("F1".into(), vec![1]), ("F2".into(), vec![2])]
    );
}

#[test]
fn literal_string_escapes_are_decoded() {
    let content = b"/F1 1 Tf (\\000\\001\\n\\(\\)\\\\\\\r\n\\101) Tj";
    // Bytes: 00 01 0A 28 29 5C 41 -> pairs 0001, 0A28, 295C; the odd one is dropped.
    assert_eq!(
        shown(content),
        vec![("F1".into(), vec![0x0001, 0x0A28, 0x295C])]
    );
}

#[test]
fn quote_operators_show_text_too() {
    let content = b"/F1 1 Tf (ab) ' 1 2 (cd) \"";
    assert_eq!(shown(content), vec![("F1".into(), vec![0x6162, 0x6364])]);
}

#[test]
fn no_codes_unless_asked() {
    assert!(super::content(b"/F1 1 Tf (ab) Tj", false)
        .shown
        .values()
        .all(|s| s.is_empty()));
}

#[test]
fn measures_nesting() {
    assert_eq!(max_nesting(b"1 begincmap"), 0);
    assert_eq!(max_nesting(b"[1 [2] <</A [3]>>]"), 3);
    assert_eq!(max_nesting(b"((a) (b (c)))"), 3);
    assert_eq!(max_nesting(b"(\\( \\( \\()"), 1);
    assert_eq!(max_nesting(b"{ { } } % [[[["), 2);
    let deep = "[".repeat(10_000);
    assert_eq!(max_nesting(deep.as_bytes()), 10_000);
    let parens = "(".repeat(5_000);
    assert_eq!(max_nesting(parens.as_bytes()), 5_000);
}

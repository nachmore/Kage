use super::*;
use encoding_rs::{SHIFT_JIS, WINDOWS_1252};

fn utf16(text: &str, little: bool) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(|u| {
            if little {
                u.to_le_bytes()
            } else {
                u.to_be_bytes()
            }
        })
        .collect()
}

fn decoded(bytes: &[u8]) -> (String, bool, bool) {
    let d = decode(bytes);
    (d.text.into_owned(), d.replaced, d.legacy)
}

#[test]
fn strict_utf8_is_borrowed() {
    let d = decode("héllo wörld".as_bytes());
    assert!(matches!(d.text, Cow::Borrowed(_)));
    assert!(!d.replaced && !d.legacy);
}

#[test]
fn boms_win() {
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend_from_slice("héllo".as_bytes());
    assert_eq!(decoded(&bytes), ("héllo".into(), false, false));

    let mut le = vec![0xFF, 0xFE];
    le.extend(utf16("Grüße 漢字", true));
    assert_eq!(decoded(&le), ("Grüße 漢字".into(), false, false));

    let mut be = vec![0xFE, 0xFF];
    be.extend(utf16("Grüße 漢字", false));
    assert_eq!(decoded(&be), ("Grüße 漢字".into(), false, false));
}

#[test]
fn utf16_without_a_bom_by_its_nul_pattern() {
    let text = "Plain notes, written by an old editor.\nSecond line.";
    assert_eq!(decoded(&utf16(text, true)), (text.into(), false, false));
    assert_eq!(decoded(&utf16(text, false)), (text.into(), false, false));
}

#[test]
fn utf8_with_a_few_bad_bytes_stays_utf8() {
    let mut bytes = "naïve café résumé déjà vu — ".repeat(10).into_bytes();
    bytes.push(0xFF);
    bytes.extend_from_slice("encore ünd".as_bytes());
    let (text, replaced, legacy) = decoded(&bytes);
    assert!(replaced && !legacy);
    assert!(text.contains("café") && text.contains('\u{FFFD}'));

    // A character cut off at the end.
    let mut cut = "élan".as_bytes().to_vec();
    cut.extend_from_slice(&"é".as_bytes()[..1]);
    let (text, replaced, legacy) = decoded(&cut);
    assert_eq!(text, "élan\u{FFFD}");
    assert!(replaced && !legacy);
}

#[test]
fn legacy_encodings_are_guessed() {
    let french = "Le garçon a mangé une crème brûlée à côté du café. Où est la fenêtre ? \
                  Été comme hiver, il préfère les pâtes fraîches.\n"
        .repeat(4);
    let (bytes, _, _) = WINDOWS_1252.encode(&french);
    let (text, _, legacy) = decoded(&bytes);
    assert!(legacy);
    assert_eq!(text, french);

    let japanese = "今日は良い天気ですね。明日の会議の資料を準備してください。\n".repeat(8);
    let (bytes, _, _) = SHIFT_JIS.encode(&japanese);
    let (text, _, legacy) = decoded(&bytes);
    assert!(legacy);
    assert_eq!(text, japanese);
}

#[test]
fn binary_looks_binary() {
    let mut bytes = b"\x7FELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00".to_vec();
    bytes.extend((0..4000u32).map(|i| (i * 7 % 256) as u8));
    assert!(looks_binary(&decode(&bytes).text));
    // Raw UTF-16 code units of small numbers look like controls.
    let words: Vec<u8> = (0..2000u16).flat_map(|i| (i % 20).to_le_bytes()).collect();
    assert!(looks_binary(&decode(&words).text));
}

#[test]
fn colour_codes_form_feeds_and_rare_controls_are_text() {
    let log = "\x1b[31merror\x1b[0m: disk full\n".repeat(200);
    assert!(!looks_binary(&log));
    assert!(!looks_binary("page one\x0cpage two\x0b\r\n"));
    let mut mostly = "x".repeat(990);
    mostly.push_str(&"\x01".repeat(9));
    assert!(!looks_binary(&mostly), "1 % is still text");
    mostly.push_str("\x01\x01");
    assert!(looks_binary(&mostly));
}

#[test]
fn only_the_first_8_kib_is_sampled() {
    let mut text = "a".repeat(SAMPLE);
    text.push_str(&"\0".repeat(1000));
    assert!(!looks_binary(&text));
}

fn job(format: Format, bytes: &[u8], max_chars: u32) -> Job<'_> {
    Job::new(format, bytes, max_chars)
}

#[test]
fn extract_reads_text() {
    let bytes = b"Line one  \r\nLine two\r\n\r\n\r\nEnd".to_vec();
    let got = extract(&job(Format::Text, &bytes, 100)).unwrap();
    assert_eq!(got.text.text, "Line one\nLine two\n\nEnd");
    assert_eq!(got.title, None, "plain text has no title");
    assert!(got.warnings.is_empty());
}

#[test]
fn extract_warns_about_decoding() {
    let french = "Déjà vu, ça va très bien. ".repeat(20);
    let (bytes, _, _) = WINDOWS_1252.encode(&french);
    let got = extract(&job(Format::Text, &bytes, 100)).unwrap();
    assert!(got.warnings.contains(&Warning::LegacyEncoding));
    let mut bytes = "é".repeat(20).into_bytes();
    bytes.extend_from_slice(b" ok \xFF");
    let got = extract(&job(Format::Text, &bytes, 100)).unwrap();
    assert_eq!(got.warnings, vec![Warning::ReplacedCharacters]);
}

#[test]
fn extract_refuses_binary() {
    let bytes: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    let err = extract(&job(Format::Text, &bytes, 100)).unwrap_err();
    assert_eq!(err.code, WorkerErrorCode::NotText);
}

#[test]
fn extract_truncates_and_counts_the_rest() {
    let text = "word ".repeat(10_000);
    let got = extract(&job(Format::Markdown, text.as_bytes(), 1_000)).unwrap();
    assert!(got.text.truncated);
    assert!(got.text.chars <= 1_000);
    assert_eq!(got.text.total_chars, Some(49_999));
}

#[test]
fn markdown_gets_its_title() {
    let md = "Intro line\n\n# Quarterly plan #\n\nBody\n";
    let got = extract(&job(Format::Markdown, md.as_bytes(), 100)).unwrap();
    assert_eq!(got.title.as_deref(), Some("Quarterly plan"));
    assert_eq!(got.text.text, md.trim_end());
}

#[test]
fn title_rules() {
    let cases: &[(&str, Option<&str>)] = &[
        ("# Plain", Some("Plain")),
        ("   # Indented three", Some("Indented three")),
        ("    # Indented four is code", None),
        ("## Second level only", None),
        ("#hashtag", None),
        ("# Closing # #", Some("Closing #")),
        ("# C# notes", Some("C# notes")),
        ("#", None),
        ("Setext title\n===\nbody", Some("Setext title")),
        ("\n===\n", None),
        ("```\n# not a title\n```\n# Real", Some("Real")),
        ("~~~~\n# no\n~~~\n# still code\n~~~~\n# Yes", Some("Yes")),
        ("\u{feff}# With a BOM", Some("With a BOM")),
        (
            "---\ntitle: \"From front matter\"\ntags: [a]\n---\n# Heading",
            Some("From front matter"),
        ),
        ("---\ntitle: 'Single'\n---\n", Some("Single")),
        (
            "---\ndate: 2024\n---\n# After front matter",
            Some("After front matter"),
        ),
        ("---\ntitle: x\n# never closed", Some("never closed")),
        ("no headings here", None),
    ];
    for (md, want) in cases {
        assert_eq!(markdown_title(md).as_deref(), *want, "{md:?}");
    }
}

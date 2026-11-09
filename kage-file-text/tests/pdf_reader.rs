//! The PDF reader: text conventions, titles, pages, truncation, warnings,
//! encryption, repair and the text fixes (EXTENSION_FILE_TEXT §3.7,
//! §5.9, §6.2, §12).

mod pdf_fixtures;

use kage_file_text::{Pages, Warning, WorkerErrorCode};
use lopdf::{dictionary, Dictionary, Object, Stream, StringFormat};
use pdf_fixtures::*;

#[test]
fn reads_pages_in_order_with_a_blank_line_between() {
    let mut pdf = Pdf::new();
    pdf.page(&["First page, first line.", "Second line."]);
    pdf.page(&["Second page."]);
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(
        got.text,
        "First page, first line.\nSecond line.\n\nSecond page."
    );
    assert_eq!(got.pages, Some(Pages { count: 2, read: 2 }));
    assert_eq!(got.page_starts, Some(vec![0, 38]));
    assert_eq!(&got.text[38..], "Second page.");
    assert_eq!(got.total_chars, Some(got.chars));
    assert!(!got.truncated);
    assert!(got.warnings.is_empty(), "{:?}", got.warnings);
}

#[test]
fn a_paragraph_gap_becomes_a_blank_line() {
    let mut pdf = Pdf::new();
    pdf.page(&["One paragraph.", "", "Another paragraph."]);
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "One paragraph.\n\nAnother paragraph.");
}

#[test]
fn the_title_comes_from_the_document_information() {
    let mut pdf = Pdf::new();
    pdf.page(&["Body text for the title test."]);
    pdf.title(Object::string_literal("Quarterly  plan\n2025"));
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.title.as_deref(), Some("Quarterly plan 2025"));

    let mut pdf = Pdf::new();
    pdf.page(&["Body text for the title test."]);
    let mut utf16 = vec![0xFE, 0xFF];
    utf16.extend(
        "Plan für 2025 – 日本"
            .encode_utf16()
            .flat_map(u16::to_be_bytes),
    );
    pdf.title(Object::String(utf16, StringFormat::Hexadecimal));
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.title.as_deref(), Some("Plan für 2025 – 日本"));
}

#[test]
fn no_title_when_there_is_none() {
    let mut pdf = Pdf::new();
    pdf.page(&["Body text without any title."]);
    assert_eq!(read(&pdf.bytes()).unwrap().title, None);
    let mut pdf = Pdf::new();
    pdf.page(&["Body text with an empty title."]);
    pdf.title(Object::string_literal("   "));
    assert_eq!(read(&pdf.bytes()).unwrap().title, None);
}

fn three_pages() -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.page(&["Alpha alpha alpha alpha alpha alpha."]);
    pdf.page(&["Bravo bravo bravo bravo bravo bravo."]);
    pdf.page(&["Charlie charlie charlie charlie charlie."]);
    pdf.bytes()
}

#[test]
fn truncation_stops_at_max_chars_and_stops_reading() {
    let bytes = three_pages();
    let all = read(&bytes).unwrap();
    let cut = read_with(&bytes, 50).unwrap();
    assert!(cut.truncated);
    assert!(cut.chars <= 50);
    assert_eq!(cut.total_chars, None, "pages after the cut weren't read");
    assert!(all.text.starts_with(&cut.text));
    // The cut prefers the paragraph break between the first two pages.
    assert!(
        cut.text.ends_with("alpha.") || cut.text.contains("Bravo"),
        "{:?}",
        cut.text
    );
    let pages = cut.pages.unwrap();
    assert_eq!(pages.count, 3);
    assert!(pages.read < 3, "stopped early: {pages:?}");
    let starts = cut.page_starts.unwrap();
    assert!(starts.iter().all(|&s| s <= cut.chars), "{starts:?}");
    assert!(!cut.warnings.contains(&Warning::NoText));
    assert!(!cut.warnings.contains(&Warning::PagesSkipped));
}

#[test]
fn truncation_at_every_size_keeps_a_prefix() {
    let bytes = three_pages();
    let all = read(&bytes).unwrap().text;
    for max in [1, 2, 7, 20, 37, 38, 39, 80, 115, 116, 117, 200] {
        let got = read_with(&bytes, max).unwrap();
        assert!(got.chars <= max, "{max}");
        assert!(
            all.starts_with(got.text.trim_end()),
            "{max}: {:?}",
            got.text
        );
        assert_eq!(got.truncated, (all.chars().count() as u32) > max, "{max}");
    }
}

#[test]
fn a_page_without_text_is_no_text() {
    let mut pdf = Pdf::new();
    let res = pdf.resources(Dictionary::new());
    pdf.page_with(b"0 0 m 100 100 l S", res);
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "");
    assert!(got.warnings.contains(&Warning::NoText));
    assert_eq!(got.pages, Some(Pages { count: 1, read: 1 }));
}

#[test]
fn little_text_per_page_is_no_text() {
    let mut pdf = Pdf::new();
    for _ in 0..4 {
        pdf.page(&["p. 1"]);
    }
    let got = read(&pdf.bytes()).unwrap();
    assert!(
        got.warnings.contains(&Warning::NoText),
        "{:?}",
        got.warnings
    );
    let mut pdf = Pdf::new();
    pdf.page(&["Enough text on this page to count as text."]);
    assert!(!read(&pdf.bytes())
        .unwrap()
        .warnings
        .contains(&Warning::NoText));
}

#[test]
fn a_page_without_a_media_box_gets_letter_size() {
    let mut pdf = Pdf::new();
    let page = pdf.page(&["No media box on this page."]);
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(page) {
        d.remove(b"MediaBox");
    }
    assert_eq!(
        read(&pdf.bytes()).unwrap().text,
        "No media box on this page."
    );
}

#[test]
fn broken_boxes_are_replaced_or_dropped() {
    let mut pdf = Pdf::new();
    let page = pdf.page(&["Broken boxes on this page."]);
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(page) {
        d.set("MediaBox", vec![0.into(), "x".into()]);
        d.set("ArtBox", vec![1.into()]);
    }
    assert_eq!(
        read(&pdf.bytes()).unwrap().text,
        "Broken boxes on this page."
    );
}

#[test]
fn resources_and_media_box_are_inherited() {
    let mut pdf = Pdf::new();
    let page = pdf.page(&["Inherited resources."]);
    let resources = pdf.resources(Dictionary::new());
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(page) {
        d.remove(b"Resources");
        d.remove(b"MediaBox");
    }
    pdf.finish();
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(pdf.pages) {
        d.set("Resources", resources);
        d.set("MediaBox", vec![0.into(), 0.into(), 300.into(), 300.into()]);
    }
    assert_eq!(
        read(&save(&mut pdf.doc)).unwrap().text,
        "Inherited resources."
    );
}

#[test]
fn a_parent_cycle_does_not_loop() {
    let mut pdf = Pdf::new();
    let page = pdf.page(&["Parent cycle page."]);
    pdf.finish();
    let middle = pdf.add(dictionary! { "Type" => "Pages", "Parent" => page });
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(page) {
        d.set("Parent", middle);
        d.remove(b"MediaBox");
    }
    assert_eq!(
        read(&save(&mut pdf.doc)).unwrap().text,
        "Parent cycle page."
    );
}

#[test]
fn a_file_with_no_pages_is_malformed() {
    let pdf = Pdf::new();
    assert_eq!(read(&pdf.bytes()).unwrap_err(), WorkerErrorCode::Malformed);
}

#[test]
fn garbage_after_the_header_is_malformed() {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    bytes.extend((0..5_000u32).map(|i| (i * 7919 % 251) as u8));
    assert_eq!(read(&bytes).unwrap_err(), WorkerErrorCode::Malformed);
    assert_eq!(read(b"%PDF-").unwrap_err(), WorkerErrorCode::Malformed);
}

#[test]
fn an_xref_only_the_repair_step_can_read() {
    let mut pdf = Pdf::new();
    pdf.page(&["Text behind a broken xref."]);
    let bytes = pdf.bytes();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let at = text.rfind("startxref").unwrap();
    let mut broken = bytes[..at].to_vec();
    broken.extend_from_slice(b"startxref\n99999999\n%%EOF\n");
    assert!(
        pdf_extract::Document::load_mem(&broken).is_err(),
        "0.42 alone fails"
    );
    assert_eq!(read(&broken).unwrap().text, "Text behind a broken xref.");
}

fn encrypted(user: &str, permissions: lopdf::Permissions) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.page(&["Secret but readable text."]);
    pdf.title(Object::string_literal("Encrypted title"));
    pdf.finish();
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
        document: &pdf.doc,
        owner_password: "owner",
        user_password: user,
        key_length: 128,
        permissions,
    })
    .unwrap();
    pdf.doc.encrypt(&state).unwrap();
    save(&mut pdf.doc)
}

#[test]
fn an_owner_password_only_file_is_read_and_restricted() {
    let bytes = encrypted("", lopdf::Permissions::PRINTABLE);
    let got = read(&bytes).unwrap();
    assert_eq!(got.text, "Secret but readable text.");
    assert_eq!(got.title.as_deref(), Some("Encrypted title"));
    assert!(got.warnings.contains(&Warning::Restricted));
}

#[test]
fn copying_allowed_is_not_restricted() {
    let bytes = encrypted("", lopdf::Permissions::all());
    let got = read(&bytes).unwrap();
    assert_eq!(got.text, "Secret but readable text.");
    assert!(!got.warnings.contains(&Warning::Restricted));
}

#[test]
fn a_user_password_file_is_encrypted() {
    let bytes = encrypted("open sesame", lopdf::Permissions::all());
    assert_eq!(read(&bytes).unwrap_err(), WorkerErrorCode::Encrypted);
}

fn aes_256(user: &str) -> Vec<u8> {
    use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
    let mut pdf = Pdf::new();
    pdf.page(&["Text under AES-256."]);
    pdf.finish();
    let filter: std::sync::Arc<dyn CryptFilter> = std::sync::Arc::new(Aes256CryptFilter);
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: std::collections::BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &[7u8; 32],
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: "owner",
        user_password: user,
        permissions: lopdf::Permissions::PRINTABLE,
    })
    .unwrap();
    pdf.doc.encrypt(&state).unwrap();
    save(&mut pdf.doc)
}

#[test]
fn aes_256_files_are_read_or_refused_like_rc4() {
    let got = read(&aes_256("")).unwrap();
    assert_eq!(got.text, "Text under AES-256.");
    assert!(got.warnings.contains(&Warning::Restricted));
    assert_eq!(
        read(&aes_256("secret")).unwrap_err(),
        WorkerErrorCode::Encrypted
    );
}

#[test]
fn ligatures_and_kangxi_radicals_are_unfolded() {
    let mut pdf = Pdf::new();
    let fonts = mapped_font(&mut pdf, &[(b'a', "\u{FB01}"), (b'b', "\u{2FB3}")]);
    let res = pdf.resources(fonts);
    let content =
        "BT /F2 12 Tf 72 720 Td (a) Tj /F1 12 Tf (le names) Tj ET BT /F2 12 Tf 72 706 Td (b) Tj ET";
    pdf.page_with(content.as_bytes(), res);
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "file names\n音");
}

#[test]
fn a_hebrew_line_comes_back_in_logical_order() {
    let mut pdf = Pdf::new();
    // Codes a..e draw ש ל ו ם ע. The PDF draws "שלום עולם" from left to
    // right, so the content holds "םלוע םולש".
    let fonts = mapped_font(
        &mut pdf,
        &[
            (b'a', "ש"),
            (b'b', "ל"),
            (b'c', "ו"),
            (b'd', "ם"),
            (b'e', "ע"),
        ],
    );
    let res = pdf.resources(fonts);
    let content = "BT /F2 12 Tf 72 720 Td (dbce dcba) Tj ET";
    pdf.page_with(content.as_bytes(), res);
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "שלום עולם");
}

#[test]
fn a_cid_font_without_to_unicode_reads_through_adobes_table() {
    let mut pdf = Pdf::new();
    let font = cid_font(&mut pdf, "Japan1");
    let res = pdf.resources(dictionary! { "Font" => dictionary! { "F3" => font } });
    // CIDs 3284, 3722 and 843: 日, 本 and あ in Adobe-Japan1.
    let content = format!(
        "BT /F3 12 Tf 72 720 Td {} ET",
        show_cids(&[3284, 3722, 843])
    );
    pdf.page_with(content.as_bytes(), res);
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "日本あ");
}

#[test]
fn each_adobe_ordering_is_known() {
    for (ordering, cids, expected) in [
        ("Korea1", vec![3296u16, 1204], "한국"),
        ("GB1", vec![4559, 3795], "中文"),
        ("CNS1", vec![661, 726], "中文"),
    ] {
        let mut pdf = Pdf::new();
        let font = cid_font(&mut pdf, ordering);
        let res = pdf.resources(dictionary! { "Font" => dictionary! { "F3" => font } });
        let content = format!("BT /F3 12 Tf 72 720 Td {} ET", show_cids(&cids));
        pdf.page_with(content.as_bytes(), res);
        assert_eq!(read(&pdf.bytes()).unwrap().text, expected, "{ordering}");
    }
}

#[test]
fn a_cid_font_with_its_own_map_keeps_it() {
    let mut pdf = Pdf::new();
    let font = cid_font(&mut pdf, "Japan1");
    let map = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
        1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
        1 beginbfchar <0CD4> <0058> endbfchar endcmap end end\n";
    let map = pdf.add(Stream::new(Dictionary::new(), map.to_vec()));
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(font) {
        d.set("ToUnicode", map);
    }
    let res = pdf.resources(dictionary! { "Font" => dictionary! { "F3" => font } });
    let content = format!("BT /F3 12 Tf 72 720 Td {} ET", show_cids(&[3284]));
    pdf.page_with(content.as_bytes(), res);
    assert_eq!(read(&pdf.bytes()).unwrap().text, "X");
}

#[test]
fn text_is_normalised() {
    let mut pdf = Pdf::new();
    // A tag character, a bidi override and a combining accent.
    let fonts = mapped_font(
        &mut pdf,
        &[(b'a', "\u{E0041}"), (b'b', "\u{202E}"), (b'c', "e\u{0301}")],
    );
    let res = pdf.resources(fonts);
    pdf.page_with(b"BT /F2 12 Tf 72 720 Td (abc) Tj ET", res);
    assert_eq!(read(&pdf.bytes()).unwrap().text, "é");
}

#[test]
fn a_panic_on_one_page_loses_only_that_page() {
    let mut pdf = Pdf::new();
    pdf.page(&["Before the broken page."]);
    // Tj with a number: pdf-extract panics on it.
    let res = pdf.resources(Dictionary::new());
    pdf.page_with(b"BT /F1 12 Tf 72 720 Td (Partial) Tj 42 Tj ET", res);
    pdf.page(&["After the broken page."]);
    let got = read(&pdf.bytes()).unwrap();
    assert!(
        got.text.starts_with("Before the broken page."),
        "{:?}",
        got.text
    );
    assert!(
        got.text.ends_with("After the broken page."),
        "{:?}",
        got.text
    );
    assert_eq!(got.total_chars, None, "a page was lost");
}

#[test]
fn every_page_panicking_is_malformed() {
    let mut pdf = Pdf::new();
    let res = pdf.resources(Dictionary::new());
    pdf.page_with(b"BT /F1 12 Tf 42 Tj ET", res);
    assert_eq!(read(&pdf.bytes()).unwrap_err(), WorkerErrorCode::Malformed);
}

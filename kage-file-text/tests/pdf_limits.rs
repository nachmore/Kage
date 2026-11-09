//! The PDF reader on hostile and huge files (EXTENSION_FILE_TEXT §5.7,
//! §6.1, §6.2, §12): bombs, cycles, nesting, page caps, and the worker
//! as a process.

mod pdf_fixtures;

use kage_core::file_text_wire::{
    write_request, Format, WorkerRequest, WorkerResponse, MAX_PAGES, WORKER_FLAG,
};
use kage_file_text::{Pages, Warning};
use lopdf::{dictionary, Dictionary, Object, ObjectId, Stream};
use pdf_fixtures::*;
use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Debug builds are slow; the release worker is several times faster.
fn generous(release: Duration) -> Duration {
    if cfg!(debug_assertions) {
        release * 20
    } else {
        release
    }
}

fn form(pdf: &mut Pdf, content: &str, resources: Option<Dictionary>) -> ObjectId {
    let mut dict = dictionary! { "Type" => "XObject", "Subtype" => "Form",
    "BBox" => vec![0.into(), 0.into(), 612.into(), 792.into()] };
    if let Some(res) = resources {
        dict.set("Resources", res);
    }
    pdf.add(Stream::new(dict, content.as_bytes().to_vec()))
}

fn set_stream(pdf: &mut Pdf, id: ObjectId, content: &str) {
    if let Ok(Object::Stream(s)) = pdf.doc.get_object_mut(id) {
        s.set_content(content.as_bytes().to_vec());
    }
}

#[test]
fn a_content_stream_bomb_costs_little() {
    let mut pdf = Pdf::new();
    let bomb = pdf.add(bomb_stream(Dictionary::new()));
    let res = pdf.resources(Dictionary::new());
    let page = pdf.page_with(b"", res);
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(page) {
        d.set("Contents", bomb);
    }
    pdf.page(&["The page after the bomb."]);
    let start = Instant::now();
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "The page after the bomb.");
    assert!(start.elapsed() < generous(Duration::from_millis(500)));
}

#[test]
fn an_image_bomb_is_never_inflated() {
    let mut pdf = Pdf::new();
    let image = pdf.add(bomb_stream(dictionary! {
        "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1,
    }));
    let res = pdf.resources(dictionary! { "XObject" => dictionary! { "Im0" => image } });
    let content = format!("q /Im0 Do Q {}", lines(&["Text next to the image."]));
    pdf.page_with(content.as_bytes(), res);
    assert_eq!(read(&pdf.bytes()).unwrap().text, "Text next to the image.");
}

#[test]
fn a_to_unicode_bomb_drops_the_map() {
    let mut pdf = Pdf::new();
    let map = pdf.add(bomb_stream(Dictionary::new()));
    let font = pdf.add(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding", "ToUnicode" => map,
    });
    let res = pdf.resources(dictionary! { "Font" => dictionary! { "F2" => font } });
    pdf.page_with(
        b"BT /F2 12 Tf 72 720 Td (Read through the encoding.) Tj ET",
        res,
    );
    assert_eq!(
        read(&pdf.bytes()).unwrap().text,
        "Read through the encoding."
    );
}

#[test]
fn an_object_stream_bomb_loses_only_its_objects() {
    let mut pdf = Pdf::new();
    pdf.page(&["Text outside the object stream."]);
    let mut dict = dictionary! { "Type" => "ObjStm", "N" => 1, "First" => 4 };
    dict.set("Filter", "FlateDecode");
    let mut body = b"99 0 ".to_vec();
    body.resize(CAP + 1, b' ');
    pdf.add(Stream::new(dict, zlib(&body)));
    assert_eq!(
        read(&pdf.bytes()).unwrap().text,
        "Text outside the object stream."
    );
}

#[test]
fn a_bomb_in_an_encrypted_file_is_capped_too() {
    let mut pdf = Pdf::new();
    let bomb = pdf.add(bomb_stream(Dictionary::new()));
    let res = pdf.resources(Dictionary::new());
    let page = pdf.page_with(b"", res);
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(page) {
        d.set("Contents", bomb);
    }
    pdf.page(&["Encrypted page after the bomb."]);
    pdf.finish();
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
        document: &pdf.doc,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions: lopdf::Permissions::all(),
    })
    .unwrap();
    pdf.doc.encrypt(&state).unwrap();
    let got = read(&save(&mut pdf.doc)).unwrap();
    assert_eq!(got.text, "Encrypted page after the bomb.");
}

#[test]
fn a_form_that_draws_itself_is_left_out() {
    let mut pdf = Pdf::new();
    let fm = form(&mut pdf, "", None);
    let res = pdf.resources(dictionary! { "XObject" => dictionary! { "Fm0" => fm } });
    let content = format!("{}/Fm0 Do", lines(&["Text around a looping form."]));
    pdf.page_with(content.as_bytes(), res.clone());
    // The form has no resources of its own, so `/Fm0` inside it is itself.
    set_stream(&mut pdf, fm, "q /Fm0 Do Q");
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "Text around a looping form.");
}

#[test]
fn a_form_drawing_itself_through_its_first_operand_is_left_out() {
    // `pdf-extract` draws the first operand of `Do`; a scan that took the
    // last one would miss this loop.
    let mut pdf = Pdf::new();
    let fm = form(&mut pdf, "", None);
    let res = pdf.resources(dictionary! { "XObject" => dictionary! { "Fm0" => fm } });
    let content = format!("{}/Fm0 /Nothing Do", lines(&["Text around the loop."]));
    pdf.page_with(content.as_bytes(), res);
    set_stream(&mut pdf, fm, "/Fm0 /Nothing Do");
    assert_eq!(read(&pdf.bytes()).unwrap().text, "Text around the loop.");
}

#[test]
fn two_forms_drawing_each_other_are_cut() {
    let mut pdf = Pdf::new();
    let a = form(&mut pdf, "", None);
    let b = form(&mut pdf, "", None);
    let xobjects = dictionary! { "A" => a, "B" => b };
    let res = pdf.resources(dictionary! { "XObject" => xobjects });
    set_stream(
        &mut pdf,
        a,
        &format!("{} /B Do", lines(&["Inside form A."])),
    );
    set_stream(&mut pdf, b, "/A Do");
    pdf.page_with(b"/A Do", res);
    assert_eq!(read(&pdf.bytes()).unwrap().text, "Inside form A.");
}

#[test]
fn forms_nested_too_deep_are_cut() {
    let mut pdf = Pdf::new();
    let mut inner: Option<ObjectId> = None;
    for i in 0..200 {
        let res = inner.map(|id| dictionary! { "XObject" => dictionary! { "Next" => id } });
        let content = if inner.is_some() { "/Next Do" } else { "" };
        let mut res = res.unwrap_or_default();
        res.set("Font", dictionary! { "F1" => pdf.font });
        let text = if i == 0 {
            lines(&["Deepest form."])
        } else {
            String::new()
        };
        inner = Some(form(&mut pdf, &format!("{text}{content}"), Some(res)));
    }
    let res = pdf.resources(dictionary! { "XObject" => dictionary! { "Next" => inner.unwrap() } });
    let content = format!("{}/Next Do", lines(&["Top of the page."]));
    pdf.page_with(content.as_bytes(), res);
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "Top of the page.");
}

#[test]
fn forms_that_fan_out_are_cut() {
    let mut pdf = Pdf::new();
    // Each level draws the one below it 16 times: 16^8 draws in all.
    let mut below = form(&mut pdf, &"x ".repeat(1000), None);
    for _ in 0..8 {
        let res = dictionary! { "XObject" => dictionary! { "D" => below } };
        below = form(&mut pdf, &"/D Do ".repeat(16), Some(res));
    }
    let res = pdf.resources(dictionary! { "XObject" => dictionary! { "Top" => below } });
    let content = format!("{}/Top Do", lines(&["Above the fan."]));
    pdf.page_with(content.as_bytes(), res);
    let start = Instant::now();
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "Above the fan.");
    assert!(start.elapsed() < generous(Duration::from_secs(1)));
}

#[test]
fn a_page_listing_one_stream_many_times_is_skipped() {
    let mut pdf = Pdf::new();
    let big = pdf.add(Stream::new(Dictionary::new(), {
        let mut c = lines(&["Repeated content."]).into_bytes();
        c.resize(1 << 20, b' ');
        c
    }));
    let res = pdf.resources(Dictionary::new());
    let page = pdf.page_with(b"", res);
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(page) {
        d.set("Contents", vec![Object::Reference(big); 200]);
    }
    pdf.page(&["The next page."]);
    let got = read(&pdf.bytes()).unwrap();
    assert_eq!(got.text, "The next page.");
    assert_eq!(got.pages, Some(Pages { count: 2, read: 2 }));
    assert_eq!(got.total_chars, None);
}

#[test]
fn deep_nesting_in_a_to_unicode_map_drops_it() {
    let mut pdf = Pdf::new();
    let mut map = to_unicode(&[(b'a', "A")]);
    map.extend("[".repeat(100_000).into_bytes());
    let map = pdf.add(Stream::new(Dictionary::new(), map));
    let font = pdf.add(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding", "ToUnicode" => map,
    });
    let res = pdf.resources(dictionary! { "Font" => dictionary! { "F2" => font } });
    pdf.page_with(b"BT /F2 12 Tf 72 720 Td (Still read.) Tj ET", res);
    assert_eq!(read(&pdf.bytes()).unwrap().text, "Still read.");
}

#[test]
fn deep_nesting_in_a_type1_program_drops_it() {
    let mut pdf = Pdf::new();
    let program = pdf.add(Stream::new(
        Dictionary::new(),
        "{".repeat(100_000).into_bytes(),
    ));
    let descriptor = pdf.add(dictionary! {
        "Type" => "FontDescriptor", "FontName" => "Helvetica", "Flags" => 32, "FontFile" => program,
    });
    let font = pdf.add(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding", "FontDescriptor" => descriptor,
    });
    let res = pdf.resources(dictionary! { "Font" => dictionary! { "F2" => font } });
    pdf.page_with(b"BT /F2 12 Tf 72 720 Td (Program dropped.) Tj ET", res);
    assert_eq!(read(&pdf.bytes()).unwrap().text, "Program dropped.");
}

fn many_pages(n: usize) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let content = pdf.add(Stream::new(
        Dictionary::new(),
        lines(&["One small page of text."]).into_bytes(),
    ));
    let res = pdf.resources(Dictionary::new());
    for _ in 0..n {
        let page = pdf.add(dictionary! {
            "Type" => "Page", "Parent" => pdf.pages, "Resources" => res.clone(),
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Contents" => content,
        });
        pdf.kids.push(page);
    }
    pdf.bytes()
}

#[test]
fn past_the_page_cap_pages_are_skipped() {
    let got = read_with(&many_pages(MAX_PAGES as usize + 1), 1_000_000).unwrap();
    assert_eq!(
        got.pages,
        Some(Pages {
            count: MAX_PAGES + 1,
            read: MAX_PAGES
        })
    );
    assert!(got.warnings.contains(&Warning::PagesSkipped));
    assert_eq!(got.page_starts.unwrap().len(), MAX_PAGES as usize);
    assert!(!got.truncated);
    assert_eq!(got.total_chars, None);
}

#[test]
fn twenty_thousand_pages_read_the_first_two_thousand_quickly() {
    let bytes = many_pages(20_000);
    let start = Instant::now();
    let got = read_with(&bytes, 1_000_000).unwrap();
    let took = start.elapsed();
    assert_eq!(got.pages.unwrap().read, MAX_PAGES);
    assert!(took < generous(Duration::from_secs(1)), "{took:?}");
}

#[test]
fn a_huge_page_stops_at_the_cap() {
    let mut pdf = Pdf::new();
    let line = "word ".repeat(2_000);
    let text: Vec<&str> = std::iter::repeat_n(line.as_str(), 300).collect();
    pdf.page(&text);
    let got = read_with(&pdf.bytes(), 1_000).unwrap();
    assert!(got.truncated);
    assert!(got.chars <= 1_000 && got.chars > 900, "{}", got.chars);
}

#[test]
fn the_worker_reads_a_pdf() {
    let mut pdf = Pdf::new();
    pdf.page(&["Through the worker process."]);
    let request = WorkerRequest::new(vec![Format::Pdf], "pdf", 1_000);
    let mut frame = Vec::new();
    write_request(&mut frame, &request, &pdf.bytes()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kage-file-text"))
        .env_clear()
        .arg(WORKER_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&frame).unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    assert!(child.wait().unwrap().success());
    match serde_json::from_slice::<WorkerResponse>(&out).unwrap() {
        WorkerResponse::Ok {
            format,
            text,
            pages,
            ..
        } => {
            assert_eq!(format, Format::Pdf);
            assert_eq!(text, "Through the worker process.");
            assert_eq!(pages, Some(Pages { count: 1, read: 1 }));
        }
        other => panic!("{other:?}"),
    }
}

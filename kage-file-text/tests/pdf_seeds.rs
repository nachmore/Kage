//! Seeds for the `pdf` fuzz target (EXTENSION_FILE_TEXT §12), built from
//! the same kind of fixtures as the reader tests. Skipped unless
//! `KAGE_FILE_TEXT_SEEDS` names a folder:
//!
//!   KAGE_FILE_TEXT_SEEDS=fuzz/corpus/pdf cargo test -p kage-file-text --test pdf_seeds
//!
//! (the path is relative to `kage-file-text/`). Each seed starts with the
//! byte that picks the fuzzer's character cap, then the file. Streams stay
//! uncompressed, so the fuzzer can see and change their syntax.

mod pdf_fixtures;

use lopdf::{dictionary, Dictionary, Object, Stream};
use pdf_fixtures::*;

/// `kage_file_text_fuzz::CAPS[6]`: the default 100,000 characters.
const CAP_BYTE: u8 = 6;

fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let mut out = Vec::new();

    let mut pdf = Pdf::new();
    pdf.page(&["First page, first line.", "", "Second paragraph."]);
    pdf.page(&["Second page."]);
    pdf.title(Object::string_literal("A title"));
    out.push(("two-pages", pdf.bytes()));

    let mut pdf = Pdf::new();
    let fonts = mapped_font(&mut pdf, &[(b'a', "\u{FB01}"), (b'b', "ש"), (b'c', "ם")]);
    let res = pdf.resources(fonts);
    pdf.page_with(
        b"BT /F2 12 Tf 72 720 Td (abc) Tj T* [(a) -120 (b)] TJ ET",
        res,
    );
    out.push(("to-unicode", pdf.bytes()));

    let mut pdf = Pdf::new();
    let font = cid_font(&mut pdf, "Japan1");
    let res = pdf.resources(dictionary! { "Font" => dictionary! { "F3" => font } });
    let content = format!(
        "BT /F3 12 Tf 72 720 Td {} ET",
        show_cids(&[3284, 3722, 843])
    );
    pdf.page_with(content.as_bytes(), res);
    out.push(("cid-font", pdf.bytes()));

    let mut pdf = Pdf::new();
    let inner = pdf.add(Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 9.into(), 9.into()] },
        lines(&["Inside a form."]).into_bytes(),
    ));
    let res = pdf.resources(dictionary! { "XObject" => dictionary! { "Fm0" => inner } });
    let content = format!("q /Fm0 Do Q {}", lines(&["Outside."]));
    pdf.page_with(content.as_bytes(), res);
    out.push(("form", pdf.bytes()));

    let mut pdf = Pdf::new();
    let page = pdf.page(&["Inherited and compressed."]);
    let packed = pdf.add(Stream::new(
        dictionary! { "Filter" => "FlateDecode" },
        zlib(lines(&["Compressed page text."]).as_bytes()),
    ));
    if let Ok(Object::Dictionary(d)) = pdf.doc.get_object_mut(page) {
        d.set("Contents", vec![packed.into()]);
        d.remove(b"MediaBox");
    }
    out.push(("flate", pdf.bytes()));

    let mut pdf = Pdf::new();
    pdf.page(&["Encrypted text."]);
    pdf.finish();
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
        document: &pdf.doc,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions: lopdf::Permissions::PRINTABLE,
    })
    .unwrap();
    pdf.doc.encrypt(&state).unwrap();
    out.push(("encrypted", save(&mut pdf.doc)));

    let mut pdf = Pdf::new();
    let res = pdf.resources(Dictionary::new());
    pdf.page_with(b"BI /W 1 /H 1 /BPC 8 /CS /G ID \x80 EI 0 0 m 9 9 l S", res);
    out.push(("no-text", pdf.bytes()));
    out
}

#[test]
fn write_fuzz_seeds() {
    let Some(dir) = std::env::var_os("KAGE_FILE_TEXT_SEEDS") else {
        eprintln!("KAGE_FILE_TEXT_SEEDS isn't set; skipping");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in seeds() {
        let mut seed = vec![CAP_BYTE];
        seed.extend(bytes);
        std::fs::write(dir.join(format!("{name}.pdf")), seed).unwrap();
    }
}

#[test]
fn every_seed_reads() {
    for (name, bytes) in seeds() {
        let got = read(&bytes);
        assert!(got.is_ok(), "{name}: {got:?}");
    }
}

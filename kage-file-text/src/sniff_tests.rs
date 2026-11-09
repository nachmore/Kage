use super::*;
use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

const DOCX: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const PPTX: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";

fn zip_of(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(method);
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn content_types(main: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/main.xml" ContentType="{main}"/>
</Types>"#
    )
    .into_bytes()
}

fn package(main: &str) -> Vec<u8> {
    zip_of(
        &[
            ("[Content_Types].xml", &content_types(main)),
            ("main.xml", b"<doc/>"),
        ],
        CompressionMethod::Deflated,
    )
}

fn code(r: Result<(), Failure>) -> Option<WorkerErrorCode> {
    r.err().map(|f| f.code)
}

fn ole(with_encrypted_package: bool) -> Vec<u8> {
    let mut bytes = OLE_MAGIC.to_vec();
    bytes.resize(512, 0);
    let name = if with_encrypted_package {
        "EncryptedPackage"
    } else {
        "WordDocument"
    };
    bytes.extend(name.encode_utf16().flat_map(u16::to_le_bytes));
    bytes.resize(2048, 0);
    bytes
}

#[test]
fn ole_files_are_encrypted_or_legacy_whatever_the_name() {
    for format in Format::ALL {
        assert_eq!(
            code(check(format, &ole(true))),
            Some(WorkerErrorCode::Encrypted)
        );
        assert_eq!(
            code(check(format, &ole(false))),
            Some(WorkerErrorCode::LegacyFormat)
        );
    }
}

#[test]
fn pdf_header_anywhere_in_the_first_kib() {
    assert_eq!(code(check(Format::Pdf, b"%PDF-1.7\n...")), None);
    let mut junk = vec![b' '; 1000];
    junk.extend_from_slice(b"%PDF-1.4");
    assert_eq!(code(check(Format::Pdf, &junk)), None);
    let mut late = vec![b' '; 1024];
    late.extend_from_slice(b"%PDF-1.4");
    assert_eq!(
        code(check(Format::Pdf, &late)),
        Some(WorkerErrorCode::WrongContent)
    );
    assert_eq!(
        code(check(Format::Pdf, b"hello")),
        Some(WorkerErrorCode::WrongContent)
    );
}

#[test]
fn docx_and_pptx_by_their_main_part() {
    assert_eq!(code(check(Format::Docx, &package(DOCX))), None);
    assert_eq!(code(check(Format::Pptx, &package(PPTX))), None);
    assert_eq!(
        code(check(Format::Docx, &package(PPTX))),
        Some(WorkerErrorCode::WrongContent)
    );
    assert_eq!(
        code(check(Format::Pptx, &package(DOCX))),
        Some(WorkerErrorCode::WrongContent)
    );
    assert_eq!(
        code(check(Format::Docx, &package(XLSX))),
        Some(WorkerErrorCode::WrongContent)
    );
    // Macro-enabled and template types, in any case.
    let dotm = "application/vnd.ms-word.template.macroEnabledTemplate.main+xml";
    assert_eq!(ooxml_kind(&package(dotm)).unwrap(), Some(Format::Docx));
    let ppsx = "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml";
    assert_eq!(ooxml_kind(&package(ppsx)).unwrap(), Some(Format::Pptx));
}

#[test]
fn content_types_name_matches_without_case() {
    let bytes = zip_of(
        &[("[content_types].XML", &content_types(DOCX))],
        CompressionMethod::Stored,
    );
    assert_eq!(ooxml_kind(&bytes).unwrap(), Some(Format::Docx));
}

#[test]
fn content_types_in_utf16() {
    let xml = String::from_utf8(content_types(DOCX))
        .unwrap()
        .replace("UTF-8", "UTF-16");
    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    let bytes = zip_of(
        &[("[Content_Types].xml", &utf16)],
        CompressionMethod::Deflated,
    );
    assert_eq!(ooxml_kind(&bytes).unwrap(), Some(Format::Docx));
}

#[test]
fn other_zips_are_wrong_content() {
    let plain = zip_of(&[("notes.txt", b"hi")], CompressionMethod::Deflated);
    assert_eq!(
        code(check(Format::Docx, &plain)),
        Some(WorkerErrorCode::WrongContent)
    );
    let empty = zip_of(&[], CompressionMethod::Stored);
    assert!(is_zip(&empty));
    assert_eq!(
        code(check(Format::Docx, &empty)),
        Some(WorkerErrorCode::WrongContent)
    );
    assert_eq!(
        code(check(Format::Docx, b"%PDF-1.7")),
        Some(WorkerErrorCode::WrongContent)
    );
}

#[test]
fn broken_packages_are_malformed() {
    let mut bytes = package(DOCX);
    let len = bytes.len();
    bytes.truncate(len - 10);
    assert_eq!(
        code(check(Format::Docx, &bytes)),
        Some(WorkerErrorCode::Malformed)
    );
    let bad_xml = zip_of(
        &[(
            "[Content_Types].xml",
            b"<Types><Override ContentType=\"x\"></Wrong>",
        )],
        CompressionMethod::Stored,
    );
    assert_eq!(
        code(check(Format::Docx, &bad_xml)),
        Some(WorkerErrorCode::Malformed)
    );
}

#[test]
fn rtf_after_whitespace() {
    assert_eq!(code(check(Format::Rtf, b"{\\rtf1\\ansi hi}")), None);
    assert_eq!(code(check(Format::Rtf, b" \r\n\t{\\rtf1}")), None);
    assert_eq!(
        code(check(Format::Rtf, b"x{\\rtf1}")),
        Some(WorkerErrorCode::WrongContent)
    );
}

#[test]
fn the_text_family_refuses_other_formats() {
    let others: [&[u8]; 4] = [
        b"%PDF-1.7\n%\xE2\xE3\xCF\xD3",
        &package(DOCX),
        b"{\\rtf1 hi}",
        b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    ];
    for format in [Format::Text, Format::Markdown, Format::Html] {
        for bytes in others {
            assert_eq!(
                code(check(format, bytes)),
                Some(WorkerErrorCode::WrongContent)
            );
        }
        assert_eq!(code(check(format, b"plain words")), None);
        // A note that mentions the PDF header isn't a PDF.
        assert_eq!(
            code(check(format, b"# PDF notes\nFiles start with %PDF-1.x")),
            None
        );
    }
}

#[test]
fn too_many_entries_is_too_complex() {
    let names: Vec<String> = (0..=MAX_ZIP_ENTRIES).map(|i| format!("f{i}")).collect();
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for name in &names {
        w.start_file(name.as_str(), opts).unwrap();
    }
    let bytes = w.finish().unwrap().into_inner();
    assert_eq!(
        declared_zip_entries(&bytes),
        Some(MAX_ZIP_ENTRIES as u64 + 1)
    );
    assert_eq!(
        code(check(Format::Docx, &bytes)),
        Some(WorkerErrorCode::TooComplex)
    );
}

#[test]
fn zip64_entry_counts_are_read() {
    // A ZIP64 end record claiming a million entries, its locator, and an
    // end record that defers to it.
    let mut bytes = b"PK\x03\x04".to_vec();
    bytes.resize(100, 0);
    let record_at = bytes.len() as u64;
    let mut record = b"PK\x06\x06".to_vec();
    record.extend(44u64.to_le_bytes());
    record.extend([0u8; 12]);
    record.extend(1_000_000u64.to_le_bytes());
    record.extend(1_000_000u64.to_le_bytes());
    record.extend([0u8; 16]);
    bytes.extend(record);
    bytes.extend(b"PK\x06\x07");
    bytes.extend(0u32.to_le_bytes());
    bytes.extend(record_at.to_le_bytes());
    bytes.extend(1u32.to_le_bytes());
    bytes.extend(b"PK\x05\x06");
    bytes.extend([0u8; 6]);
    bytes.extend(u16::MAX.to_le_bytes());
    bytes.extend([0xFFu8; 8]);
    bytes.extend(0u16.to_le_bytes());
    assert_eq!(declared_zip_entries(&bytes), Some(1_000_000));
    assert_eq!(
        code(check(Format::Docx, &bytes)),
        Some(WorkerErrorCode::TooComplex)
    );
    assert_eq!(declared_zip_entries(b"PK\x05\x06"), None);
}

/// Sets a `u32` field in every central directory header.
fn patch_central(bytes: &mut [u8], offset: usize, f: impl Fn(u32) -> u32) {
    let mut i = 0;
    while let Some(p) = memchr::memmem::find(&bytes[i..], b"PK\x01\x02") {
        let at = i + p + offset;
        let old = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        bytes[at..at + 4].copy_from_slice(&f(old).to_le_bytes());
        i += p + 4;
    }
}

#[test]
fn encrypted_entries_are_encrypted() {
    let mut bytes = package(DOCX);
    // General purpose flags (with the version field before them): set the
    // "encrypted" bit.
    patch_central(&mut bytes, 6, |v| v | (1 << 16));
    assert_eq!(
        code(check(Format::Docx, &bytes)),
        Some(WorkerErrorCode::Encrypted)
    );
}

#[test]
fn an_entry_that_inflates_past_its_size_is_malformed() {
    let mut bytes = zip_of(&[("a.xml", &[b'x'; 1000])], CompressionMethod::Stored);
    patch_central(&mut bytes, 24, |_| 10);
    let mut archive = open_package(&bytes).unwrap();
    let err = read_entry(&mut archive, 0, 1 << 20).unwrap_err();
    assert_eq!(err.code, WorkerErrorCode::Malformed);
}

#[test]
fn an_entry_past_the_cap_is_too_complex() {
    let bytes = zip_of(
        &[("a.xml", &vec![0u8; 1 << 20])],
        CompressionMethod::Deflated,
    );
    assert!(bytes.len() < 1 << 14, "a high-ratio entry");
    let mut archive = open_package(&bytes).unwrap();
    assert_eq!(read_entry(&mut archive, 0, 1 << 20).unwrap().len(), 1 << 20);
    let err = read_entry(&mut archive, 0, 64 * 1024).unwrap_err();
    assert_eq!(err.code, WorkerErrorCode::TooComplex);
}

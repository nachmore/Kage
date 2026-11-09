use super::*;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Cursor;

#[test]
fn format_ids_are_the_serde_names_and_round_trip() {
    for f in Format::ALL {
        assert_eq!(serde_json::to_value(f).unwrap(), json!(f.id()));
        assert_eq!(Format::from_id(f.id()), Some(f));
        let back: Format = serde_json::from_value(json!(f.id())).unwrap();
        assert_eq!(back, f);
    }
    assert_eq!(Format::from_id("PDF"), None);
    assert!(serde_json::from_value::<Format>(json!("doc")).is_err());
}

#[test]
fn extension_table_matches_the_design() {
    let table: Vec<(&str, Vec<&str>, bool)> = Format::ALL
        .into_iter()
        .map(|f| (f.id(), f.extensions().to_vec(), f.is_p1()))
        .collect();
    assert_eq!(
        table,
        vec![
            ("text", vec!["txt", "text", "log", "csv", "tsv"], false),
            ("markdown", vec!["md", "markdown", "mdown", "mkd"], false),
            ("pdf", vec!["pdf"], false),
            ("docx", vec!["docx"], false),
            ("html", vec!["html", "htm", "xhtml"], true),
            ("pptx", vec!["pptx"], true),
            ("rtf", vec!["rtf"], true),
        ]
    );
}

#[test]
fn extensions_are_unique_lowercase_and_dotless() {
    let mut seen = HashSet::new();
    for f in Format::ALL {
        for e in f.extensions() {
            assert!(seen.insert(*e), "{e} listed twice");
            assert!(
                e.chars().all(|c| c.is_ascii_lowercase()),
                "{e} must be lowercase a-z only"
            );
            assert_eq!(Format::from_extension(e), Some(f));
        }
    }
}

#[test]
fn from_extension_ignores_case_but_not_the_dot() {
    assert_eq!(Format::from_extension("PDF"), Some(Format::Pdf));
    assert_eq!(Format::from_extension("Md"), Some(Format::Markdown));
    assert_eq!(Format::from_extension(".pdf"), None);
    assert_eq!(Format::from_extension("doc"), None);
    assert_eq!(Format::from_extension(""), None);
}

#[test]
fn readable_formats_follow_the_p1_switch() {
    let readable: Vec<Format> = readable_formats().collect();
    let first = vec![Format::Text, Format::Markdown, Format::Pdf, Format::Docx];
    if READS_P1_FORMATS {
        assert_eq!(readable, Format::ALL.to_vec());
    } else {
        assert_eq!(readable, first);
    }
}

#[test]
fn warnings_and_error_codes_have_stable_wire_names() {
    let warnings = [
        (Warning::PagesSkipped, "pages_skipped"),
        (Warning::NoText, "no_text"),
        (Warning::ReplacedCharacters, "replaced_characters"),
        (Warning::LegacyEncoding, "legacy_encoding"),
        (Warning::Restricted, "restricted"),
        (Warning::HiddenTextSkipped, "hidden_text_skipped"),
        (Warning::PossibleSecret, "possible_secret"),
    ];
    for (w, wire) in warnings {
        assert_eq!(serde_json::to_value(w).unwrap(), json!(wire));
        assert_eq!(serde_json::from_value::<Warning>(json!(wire)).unwrap(), w);
    }
    let codes = [
        WorkerErrorCode::Unsupported,
        WorkerErrorCode::WrongContent,
        WorkerErrorCode::Encrypted,
        WorkerErrorCode::LegacyFormat,
        WorkerErrorCode::NotText,
        WorkerErrorCode::Malformed,
        WorkerErrorCode::TooComplex,
    ];
    for c in codes {
        assert_eq!(serde_json::to_value(c).unwrap(), json!(c.as_str()));
        assert_eq!(
            serde_json::from_value::<WorkerErrorCode>(json!(c.as_str())).unwrap(),
            c
        );
    }
}

#[test]
fn request_json_shape_and_round_trip() {
    let req = WorkerRequest::new(vec![Format::Pdf, Format::Docx], "pdf", DEFAULT_MAX_CHARS);
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(
        v,
        json!({
            "v": 1, "formats": ["pdf", "docx"], "extension": "pdf",
            "max_chars": 100_000, "max_pages": 2_000, "max_slides": 1_000,
        })
    );
    assert_eq!(serde_json::from_value::<WorkerRequest>(v).unwrap(), req);
}

#[test]
fn ok_response_json_shape_and_round_trip() {
    let resp = WorkerResponse::Ok {
        format: Format::Pdf,
        text: "Page one\n\nPage two".into(),
        chars: 18,
        truncated: false,
        total_chars: Some(18),
        title: Some("Plan".into()),
        pages: Some(Pages { count: 2, read: 2 }),
        page_starts: Some(vec![0, 10]),
        warnings: vec![Warning::Restricted],
    };
    let v: Value = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["format"], "pdf");
    assert_eq!(v["pages"], json!({ "count": 2, "read": 2 }));
    assert_eq!(v["warnings"], json!(["restricted"]));
    assert_eq!(serde_json::from_value::<WorkerResponse>(v).unwrap(), resp);
}

#[test]
fn ok_response_may_leave_out_optional_fields() {
    let v = json!({ "status": "ok", "format": "text", "text": "", "chars": 0, "truncated": false });
    let resp: WorkerResponse = serde_json::from_value(v).unwrap();
    assert_eq!(
        resp,
        WorkerResponse::Ok {
            format: Format::Text,
            text: String::new(),
            chars: 0,
            truncated: false,
            total_chars: None,
            title: None,
            pages: None,
            page_starts: None,
            warnings: vec![],
        }
    );
}

#[test]
fn err_response_json_shape_and_round_trip() {
    let resp = WorkerResponse::Err {
        code: WorkerErrorCode::TooComplex,
        detail: "xml depth 257".into(),
    };
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(
        v,
        json!({ "status": "err", "code": "too_complex", "detail": "xml depth 257" })
    );
    assert_eq!(serde_json::from_value::<WorkerResponse>(v).unwrap(), resp);
    assert!(serde_json::from_value::<WorkerResponse>(json!({ "status": "maybe" })).is_err());
}

#[test]
fn frame_round_trips() {
    let req = WorkerRequest::new(vec![Format::Text], "txt", 10);
    let payload = b"hello\0world".to_vec();
    let mut buf = Vec::new();
    write_request(&mut buf, &req, &payload).unwrap();
    let header_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    assert_eq!(&buf[4..4 + header_len], serde_json::to_vec(&req).unwrap());
    let (back, bytes) = read_request(&mut Cursor::new(buf)).unwrap();
    assert_eq!(back, req);
    assert_eq!(bytes, payload);
}

#[test]
fn frame_refuses_lengths_over_the_caps_before_allocating() {
    let mut big_header = (MAX_HEADER_BYTES + 1).to_le_bytes().to_vec();
    big_header.extend_from_slice(b"{}");
    let err = read_request(&mut Cursor::new(big_header)).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);

    let header = serde_json::to_vec(&WorkerRequest::new(vec![], "txt", 1)).unwrap();
    let mut big_payload = (header.len() as u32).to_le_bytes().to_vec();
    big_payload.extend_from_slice(&header);
    big_payload.extend_from_slice(&(MAX_INPUT_BYTES + 1).to_le_bytes());
    let err = read_request(&mut Cursor::new(big_payload)).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn write_refuses_a_header_over_the_cap() {
    let req = WorkerRequest::new(vec![Format::Text], "x".repeat(MAX_HEADER_BYTES as usize), 1);
    let mut buf = Vec::new();
    let err = write_request(&mut buf, &req, b"").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(buf.is_empty(), "nothing is written for a refused frame");
}

#[test]
fn frame_refuses_a_header_that_is_not_a_request() {
    let mut buf = 2u32.to_le_bytes().to_vec();
    buf.extend_from_slice(b"{}");
    buf.extend_from_slice(&0u64.to_le_bytes());
    let err = read_request(&mut Cursor::new(buf)).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn limits_match_the_design() {
    assert_eq!(MAX_INPUT_BYTES, 33_554_432);
    assert_eq!(MAX_HEADER_BYTES, 4096);
    assert_eq!(WORKER_ALLOC_CAP_BYTES, 768 * 1024 * 1024);
    assert_eq!(worker_stdout_cap(MAX_CHARS), 4_000_000 + 1_048_576);
}

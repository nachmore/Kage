//! The request from dialog to result: phases, cancels, errors after the
//! dialog, the result's fields, `possible_secret`, and the logs.

use super::*;
use crate::file_text::post::{self, Phase};
use crate::file_text::read;
use crate::permission_audit::AuditEvent;
use kage_core::file_text_wire::{Pages, Warning};

pub(super) fn temp_file(name: &str, contents: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, contents).unwrap();
    (dir, path)
}

pub(super) fn extraction(text: &str) -> Extraction {
    Extraction {
        format: Format::Text,
        text: text.into(),
        chars: text.chars().count() as u32,
        truncated: false,
        total_chars: None,
        title: None,
        pages: None,
        page_starts: None,
        warnings: Vec::new(),
    }
}

#[test]
fn a_picked_file_comes_back_as_text_without_its_path() {
    let (dir, path) = temp_file("Q3\u{202E} notes.TXT", b"Hello\nworld");
    let slot = Arc::new(Slot::default());
    let events = Recorder::default();
    let req = request("mindmap", &[Format::Text, Format::Markdown]);
    let got = run_on(
        &slot,
        &FixedDialog::picks(&path),
        &Fake::Echo,
        &events,
        &req,
    )
    .unwrap()
    .unwrap();

    assert_eq!(got.name, "Q3 notes.TXT");
    assert_eq!(got.extension, "txt");
    assert_eq!(got.format, Format::Text);
    assert_eq!(got.text, "Hello\nworld");
    assert_eq!(got.chars, 11);
    assert_eq!(got.size, 11);
    assert!(got.modified_at.is_some());
    assert_eq!(
        got.sha256,
        crate::file_text::file_ref::sha256_hex(b"Hello\nworld")
    );
    assert!(got.file_ref.starts_with("fr1_"));
    assert!(got.warnings.is_empty());
    let json = serde_json::to_string(&got).unwrap();
    assert!(!json.contains(&*dir.path().to_string_lossy()));
    for field in [
        "modified_at",
        "total_chars",
        "page_starts",
        "file_ref",
        "sha256",
    ] {
        assert!(json.contains(&format!("\"{field}\"")), "{field}");
    }

    assert_eq!(
        events.phases(),
        vec![
            Phase::DialogClosed,
            Phase::Reading,
            Phase::Extracting,
            Phase::Done
        ]
    );
    let progress = events.progress.lock().unwrap().clone();
    assert_eq!(progress[0].picked, Some(true));
    assert_eq!(progress[1].file.as_deref(), Some("Q3 notes.TXT"));
    let reports = events.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].outcome, "ok");
    assert_eq!(reports[0].chars, 11);
    assert_eq!(reports[0].format, Some(Format::Text));

    // The slot is free again, and the gap holds for this extension only.
    assert_eq!(
        slot.take("mindmap", Instant::now()).unwrap_err(),
        Code::TooSoon
    );
    drop(slot.take("other", Instant::now()).unwrap());
}

#[test]
fn closing_the_dialog_without_a_file_reads_nothing_and_writes_no_audit() {
    let slot = Arc::new(Slot::default());
    let events = Recorder::default();
    let dialog = FixedDialog {
        path: None,
        on_pick: None,
        fail: false,
    };
    let req = request("mindmap", &[Format::Text]);
    assert_eq!(
        run_on(&slot, &dialog, &Fake::Echo, &events, &req).unwrap(),
        None
    );
    assert_eq!(events.phases(), vec![Phase::DialogClosed]);
    assert_eq!(events.progress.lock().unwrap()[0].picked, Some(false));
    assert!(events.reports().is_empty());
    assert_eq!(
        slot.take("mindmap", Instant::now()).unwrap_err(),
        Code::TooSoon
    );
}

#[test]
fn a_failed_dialog_is_dialog_failed_and_writes_no_audit() {
    let slot = Arc::new(Slot::default());
    let events = Recorder::default();
    let dialog = FixedDialog {
        path: None,
        on_pick: None,
        fail: true,
    };
    let e = run_on(
        &slot,
        &dialog,
        &Fake::Echo,
        &events,
        &request("m", &[Format::Text]),
    )
    .unwrap_err();
    assert_eq!(e.key, "errors.file_text.dialog_failed");
    assert!(events.reports().is_empty());
}

#[test]
fn a_cancel_or_a_lost_grant_during_the_dialog_reads_nothing() {
    let (_dir, path) = temp_file("a.txt", b"text");
    let slot = Arc::new(Slot::default());
    let events = Recorder::default();
    let cancelling = slot.clone();
    let dialog = FixedDialog {
        path: Some(path.clone()),
        on_pick: Some(Box::new(move || assert!(cancelling.cancel("mindmap")))),
        fail: false,
    };
    let req = request("mindmap", &[Format::Text]);
    assert_eq!(
        run_on(&slot, &dialog, &Fake::Echo, &events, &req).unwrap(),
        None
    );
    assert_eq!(events.phases(), vec![Phase::DialogClosed]);
    assert!(events.reports().is_empty());

    let slot = Arc::new(Slot::default());
    let events = Recorder::default();
    events.denied.store(true, Ordering::SeqCst);
    let got = run_on(
        &slot,
        &FixedDialog::picks(&path),
        &Fake::Echo,
        &events,
        &req,
    );
    assert_eq!(got.unwrap(), None);
    assert!(events.reports().is_empty());
}

#[test]
fn a_cancel_during_the_read_or_the_extraction_is_cancelled() {
    let (_dir, path) = temp_file("a.txt", b"text");
    for phase in [Phase::Reading, Phase::Extracting] {
        let slot = Arc::new(Slot::default());
        let cancelling = slot.clone();
        let events = Recorder {
            on_progress: Some(Box::new(move |p| {
                if p.phase == phase {
                    cancelling.cancel("mindmap");
                }
            })),
            ..Default::default()
        };
        let req = request("mindmap", &[Format::Text]);
        let e = run_on(
            &slot,
            &FixedDialog::picks(&path),
            &Fake::Echo,
            &events,
            &req,
        )
        .unwrap_err();
        assert_eq!(e.key, "errors.file_text.cancelled", "{phase:?}");
        assert_eq!(e.params["name"], "a.txt");
        assert_eq!(events.reports()[0].outcome, "cancelled");
        assert_eq!(events.phases().last(), Some(&Phase::Done));
    }
}

#[test]
fn errors_after_the_dialog_are_audited_and_end_with_done() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("pages.txt");
    std::fs::create_dir(&folder).unwrap();
    let md = dir.path().join("notes.md");
    std::fs::write(&md, "# hi").unwrap();
    let big = dir.path().join("big.txt");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(wire::MAX_INPUT_BYTES + 1)
        .unwrap();
    let plain = dir.path().join("plain.txt");
    std::fs::write(&plain, "x").unwrap();

    let cases: [(&Path, Fake, &str); 6] = [
        (&folder, Fake::Echo, "not_a_file"),
        (&md, Fake::Echo, "unsupported"),
        (&big, Fake::Echo, "too_large"),
        (
            &plain,
            Fake::Fail(WorkerFailure::Refused(WorkerErrorCode::NotText)),
            "not_text",
        ),
        (
            &plain,
            Fake::Fail(WorkerFailure::Crashed("signal 9".into())),
            "too_complex",
        ),
        (
            &plain,
            Fake::Fail(WorkerFailure::Failed("bad output")),
            "worker_failed",
        ),
    ];
    for (path, fake, outcome) in cases {
        let slot = Arc::new(Slot::default());
        let events = Recorder::default();
        let req = request("mindmap", &[Format::Text]);
        let e = run_on(&slot, &FixedDialog::picks(path), &fake, &events, &req).unwrap_err();
        assert_eq!(outcome_of(&e), outcome);
        let reports = events.reports();
        assert_eq!(reports.len(), 1, "{outcome}");
        assert_eq!(reports[0].outcome, outcome);
        assert_eq!(reports[0].chars, 0);
        assert_eq!(events.phases().last(), Some(&Phase::Done));
        if outcome == "too_large" {
            assert_eq!(reports[0].bytes, wire::MAX_INPUT_BYTES + 1);
            assert_eq!(events.phases(), vec![Phase::DialogClosed, Phase::Done]);
        }
    }
}

#[test]
fn text_that_looks_like_a_secret_is_flagged_and_kept() {
    let (_dir, path) = temp_file("creds.txt", b"db password: hunter2-hunter2");
    let events = Recorder::default();
    let req = request("mindmap", &[Format::Text]);
    let got = run_on(
        &Arc::default(),
        &FixedDialog::picks(&path),
        &Fake::Echo,
        &events,
        &req,
    )
    .unwrap()
    .unwrap();
    assert_eq!(got.warnings, vec![Warning::PossibleSecret]);
    assert!(got.text.contains("hunter2"));

    let (_dir, path) = temp_file("plain.txt", b"Quarterly plan, revision 2024-Q3.");
    let got = run_on(
        &Arc::default(),
        &FixedDialog::picks(&path),
        &Fake::Echo,
        &events,
        &req,
    )
    .unwrap()
    .unwrap();
    assert!(got.warnings.is_empty());
}

#[test]
fn the_secret_check_takes_linear_time() {
    // A key marker on each of many header lines made the check quadratic:
    // about a minute for a 1,000,000-character text, in Kage's own process,
    // holding the slot, where no deadline or cancel applies.
    let line = "X: -----BEGIN\n";
    let text = line.repeat(150_000 / line.len());
    let named = read::Named {
        name: "keys.txt".into(),
        extension: "txt".into(),
        format: Some(Format::Text),
    };
    let checked = read::Checked {
        format: Format::Text,
        size: text.len() as u64,
        modified_at: None,
        placeholder: false,
    };
    let started = std::time::Instant::now();
    let got = post::build(
        &named,
        &checked,
        extraction(&text),
        "00".into(),
        "fr1_x".into(),
        wire::MAX_CHARS,
    )
    .unwrap();
    let took = started.elapsed();
    assert_eq!(got.warnings, vec![Warning::PossibleSecret]);
    assert!(took < Duration::from_secs(1), "took {took:?}");
}

#[test]
fn the_result_rechecks_what_the_worker_said() {
    let named = read::Named {
        name: "a.pdf".into(),
        extension: "pdf".into(),
        format: Some(Format::Pdf),
    };
    let checked = read::Checked {
        format: Format::Pdf,
        size: 10,
        modified_at: None,
        placeholder: false,
    };
    let build = |out: Extraction, max: u32| {
        post::build(&named, &checked, out, "00".into(), "fr1_x".into(), max)
    };
    assert_eq!(
        build(extraction("abcdef"), 5).unwrap_err().key,
        "errors.file_text.worker_failed"
    );
    let mut out = extraction("héllo");
    out.chars = 99;
    out.total_chars = Some(3);
    out.page_starts = Some(vec![0, 9]);
    out.warnings = vec![Warning::PossibleSecret, Warning::NoText, Warning::NoText];
    out.title = Some(" Big\u{2066}\n Plan ".into());
    out.pages = Some(Pages { count: 2, read: 2 });
    let got = build(out, 100).unwrap();
    assert_eq!(got.chars, 5);
    assert_eq!(got.total_chars, None);
    assert_eq!(got.page_starts, None);
    assert_eq!(got.warnings, vec![Warning::NoText]);
    assert_eq!(got.title.as_deref(), Some("Big Plan"));
    assert_eq!(got.pages, Some(Pages { count: 2, read: 2 }));

    let mut out = extraction("hello");
    out.page_starts = Some(vec![0, 3, 5]);
    out.title = Some("token: abcdefgh12345678".into());
    let got = build(out, 100).unwrap();
    assert_eq!(got.page_starts, Some(vec![0, 3, 5]));
    assert_eq!(got.warnings, vec![Warning::PossibleSecret]);
}

#[test]
fn names_and_titles_are_cleaned() {
    let clean = |s: &str| post::clean_line(s, 10);
    assert_eq!(clean("  a \t b\n\nc  "), "a b c");
    assert_eq!(
        clean("x\u{0}\u{7f}\u{85}y\u{feff}\u{e0041}z\u{fffe}"),
        "x yz"
    );
    assert_eq!(clean("\u{202B}abc\u{202C}\u{200E}"), "abc");
    assert_eq!(clean("abcdefghijKLM"), "abcdefghij");
    assert_eq!(clean("abcdefghi jk"), "abcdefghi");
    assert_eq!(clean("e\u{301}"), "\u{e9}");
}

#[test]
fn reports_carry_counts_and_the_audit_its_name() {
    let report = |name: &str, outcome: &str, bytes: u64, chars: u32| post::Report {
        extension_id: "mindmap".into(),
        name: name.into(),
        format: Some(Format::Pdf),
        bytes,
        chars,
        truncated: true,
        outcome: outcome.into(),
        duration: Duration::from_millis(1234),
    };
    let r = report("Q3 plan.pdf", "ok", 2 * 1024 * 1024, 50_000);
    let AuditEvent::ExtensionFileRead {
        extension_id,
        file_name,
        format,
        bytes,
        chars,
        truncated,
        outcome,
        duration_ms,
    } = r.audit_event()
    else {
        panic!("wrong audit event");
    };
    assert_eq!(extension_id, "mindmap");
    assert_eq!(file_name.as_deref(), Some("Q3 plan.pdf"));
    assert_eq!(format.as_deref(), Some("pdf"));
    assert_eq!((bytes, chars, truncated), (2 * 1024 * 1024, 50_000, true));
    assert_eq!((outcome.as_str(), duration_ms), ("ok", 1234));
    assert!(!r.audit_event().summary().contains("Q3 plan"));

    let secret = report("password=hunter2.txt", "too_large", 1, 0).audit_event();
    let AuditEvent::ExtensionFileRead { file_name, .. } = &secret else {
        panic!("wrong audit event");
    };
    assert_eq!(file_name, &None);
    assert_eq!(
        secret.summary(),
        "Extension 'mindmap' couldn't read a file (too_large)"
    );

    assert_eq!(
        r.telemetry_props(),
        serde_json::json!({
            "extension_id": "mindmap", "format": "pdf", "outcome": "ok",
            "size_bucket": "<10MB", "chars_bucket": "<100k", "ms_bucket": "<2000",
        })
    );
    assert!(!r.log_line().contains("Q3 plan"));
}

#[test]
fn buckets_have_the_documented_edges() {
    use post::{chars_bucket, ms_bucket, size_bucket};
    let kb = 1024;
    assert_eq!(size_bucket(100 * kb - 1), "<100KB");
    assert_eq!(size_bucket(100 * kb), "<1MB");
    assert_eq!(size_bucket(kb * kb), "<10MB");
    assert_eq!(size_bucket(10 * kb * kb), "≤32MB");
    assert_eq!(size_bucket(wire::MAX_INPUT_BYTES), "≤32MB");
    assert_eq!(size_bucket(wire::MAX_INPUT_BYTES + 1), ">32MB");
    assert_eq!(chars_bucket(9_999), "<10k");
    assert_eq!(chars_bucket(99_999), "<100k");
    assert_eq!(chars_bucket(999_999), "<1M");
    assert_eq!(chars_bucket(1_000_000), "1M");
    assert_eq!(ms_bucket(Duration::from_millis(499)), "<500");
    assert_eq!(ms_bucket(Duration::from_millis(1999)), "<2000");
    assert_eq!(ms_bucket(Duration::from_millis(9999)), "<10000");
    assert_eq!(ms_bucket(Duration::from_secs(10)), "10000+");
}

#[test]
fn progress_serialises_with_only_the_fields_it_has() {
    let p = post::Progress {
        extension_id: "m".into(),
        phase: Phase::DialogClosed,
        file: None,
        picked: Some(true),
    };
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        serde_json::json!({ "extension_id": "m", "phase": "dialog_closed", "picked": true })
    );
    let p = post::Progress {
        phase: Phase::Downloading,
        file: Some("a.pdf".into()),
        picked: None,
        ..p
    };
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        serde_json::json!({ "extension_id": "m", "phase": "downloading", "file": "a.pdf" })
    );
}

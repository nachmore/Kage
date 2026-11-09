//! Checks before the dialog, the slot, and the request flow with a fixed
//! path and a stand-in worker (docs/design/EXTENSION_FILE_TEXT.md §12).
//! The real worker process is tested in `tests/file_text_test.rs`.

use super::dialog::{DialogRequest, DialogSource};
use super::post::{Events, Phase, Progress, Report};
use super::slot::Slot;
use super::worker::{Extraction, Extractor, WorkerFailure};
use super::*;
use crate::config::ExtensionGrant;
use kage_core::file_text_wire::{WorkerErrorCode, WorkerRequest, DEFAULT_MAX_CHARS};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[path = "tests_flow.rs"]
mod flow;
#[path = "tests_logs.rs"]
mod logs;

// --- Stand-ins -------------------------------------------------------------------

type Hook = Box<dyn Fn() + Send + Sync>;

/// A dialog that "picks" a fixed path (or nothing), running `on_pick`
/// while it is open.
pub(super) struct FixedDialog {
    pub path: Option<PathBuf>,
    pub on_pick: Option<Hook>,
    pub fail: bool,
}

impl FixedDialog {
    pub fn picks(path: &Path) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            on_pick: None,
            fail: false,
        }
    }
}

impl DialogSource for FixedDialog {
    fn pick(&self, _request: &DialogRequest) -> Result<Option<PathBuf>, AppError> {
        if let Some(hook) = &self.on_pick {
            hook();
        }
        if self.fail {
            return Err(Code::DialogFailed.error(&[]));
        }
        Ok(self.path.clone())
    }
}

/// A worker that echoes the bytes as text, or fails as told. A set cancel
/// flag ends it the way a killed worker does.
#[derive(Clone)]
pub(super) enum Fake {
    Echo,
    Answer(Extraction),
    Fail(WorkerFailure),
}

impl Extractor for Fake {
    fn extract(
        &self,
        request: &WorkerRequest,
        bytes: &[u8],
        cancel: &AtomicBool,
    ) -> Result<Extraction, WorkerFailure> {
        if cancel.load(Ordering::SeqCst) {
            return Err(WorkerFailure::Cancelled);
        }
        match self {
            Fake::Echo => {
                let text = String::from_utf8_lossy(bytes).into_owned();
                Ok(Extraction {
                    format: Format::from_extension(&request.extension).unwrap(),
                    chars: text.chars().count() as u32,
                    text,
                    truncated: false,
                    total_chars: None,
                    title: None,
                    pages: None,
                    page_starts: None,
                    warnings: Vec::new(),
                })
            }
            Fake::Answer(a) => Ok(a.clone()),
            Fake::Fail(f) => Err(f.clone()),
        }
    }
}

type ProgressHook = Box<dyn Fn(&Progress) + Send + Sync>;

/// Records progress and reports.
#[derive(Default)]
pub(super) struct Recorder {
    pub progress: Mutex<Vec<Progress>>,
    pub reports: Mutex<Vec<Report>>,
    pub denied: AtomicBool,
    pub on_progress: Option<ProgressHook>,
}

impl Recorder {
    pub fn phases(&self) -> Vec<Phase> {
        self.progress
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.phase)
            .collect()
    }
    pub fn reports(&self) -> Vec<Report> {
        self.reports.lock().unwrap().clone()
    }
}

impl Events for Recorder {
    fn progress(&self, progress: &Progress) {
        if let Some(hook) = &self.on_progress {
            hook(progress);
        }
        self.progress.lock().unwrap().push(progress.clone());
    }
    fn finished(&self, report: &Report) {
        self.reports.lock().unwrap().push(report.clone());
    }
    fn still_allowed(&self) -> bool {
        !self.denied.load(Ordering::SeqCst)
    }
}

pub(super) fn request(id: &str, formats: &[Format]) -> Request {
    Request {
        extension_id: id.into(),
        display_name: "Test".into(),
        formats: formats.to_vec(),
        max_chars: DEFAULT_MAX_CHARS,
    }
}

/// Runs one request on its own slot.
pub(super) fn run_on(
    slot: &Arc<Slot>,
    dialog: &dyn DialogSource,
    extractor: &dyn Extractor,
    events: &Recorder,
    request: &Request,
) -> Result<Option<FileText>, AppError> {
    let guard = slot.take(&request.extension_id, Instant::now()).unwrap();
    run(
        &request::Env {
            dialog,
            extractor,
            events,
        },
        guard,
        request,
    )
}

fn config_with(id: &str, caps: &[&str], enabled: Option<bool>) -> Config {
    let mut config = Config::default();
    config.extension_grants.insert(
        id.into(),
        ExtensionGrant {
            granted: caps.iter().map(|c| c.to_string()).collect(),
            ..Default::default()
        },
    );
    if let Some(on) = enabled {
        config.extension_states.insert(id.into(), on);
    }
    config
}

// --- Support, errors, arguments ---------------------------------------------------

#[test]
fn support_lists_only_what_this_build_reads() {
    let s = support();
    let ids: Vec<Format> = s.formats.iter().map(|f| f.id).collect();
    assert_eq!(ids, wire::readable_formats().collect::<Vec<_>>());
    assert!(ids.contains(&Format::Pdf) && ids.contains(&Format::Docx));
    assert_eq!(ids.contains(&Format::Html), wire::READS_P1_FORMATS);
    assert_eq!(
        s.formats[0].extensions,
        vec!["txt", "text", "log", "csv", "tsv"]
    );
    assert_eq!(s.max_input_bytes, 33_554_432);
    assert_eq!(s.max_chars, 1_000_000);
    assert_eq!(s.default_max_chars, 100_000);
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["formats"][0]["id"], "text");
}

#[test]
fn every_rust_key_exists_in_the_english_catalog_with_its_kind() {
    let catalog: serde_json::Value =
        serde_json::from_str(include_str!("../../locales/en/messages.json")).unwrap();
    let kinds = [
        (Code::NotGranted, "not_permitted"),
        (Code::ExtensionDisabled, "not_permitted"),
        (Code::InvalidFormats, "invalid_input"),
        (Code::InvalidMaxChars, "invalid_input"),
        (Code::Busy, "rate_limited"),
        (Code::TooSoon, "rate_limited"),
        (Code::Unavailable, "internal"),
        (Code::DialogFailed, "internal"),
        (Code::NotAFile, "invalid_input"),
        (Code::Unsupported, "invalid_input"),
        (Code::WrongContent, "invalid_input"),
        (Code::TooLarge, "invalid_input"),
        (Code::ReadFailed, "internal"),
        (Code::NotDownloaded, "timeout"),
        (Code::Encrypted, "invalid_input"),
        (Code::LegacyFormat, "invalid_input"),
        (Code::NotText, "invalid_input"),
        (Code::Malformed, "invalid_input"),
        (Code::TooComplex, "invalid_input"),
        (Code::Timeout, "timeout"),
        (Code::Cancelled, "cancelled"),
        (Code::WorkerFailed, "internal"),
    ];
    for (code, kind) in kinds {
        assert!(
            catalog.get(code.key()).is_some(),
            "{} is missing from locales/en",
            code.key()
        );
        assert_eq!(serde_json::to_value(code.kind()).unwrap(), kind, "{code:?}");
        assert_eq!(outcome_of(&code.error(&[])), code.as_str());
    }
    for key in ["file_text.dialog.title", "file_text.dialog.filter"] {
        assert!(catalog.get(key).is_some(), "{key}");
    }
}

#[test]
fn sizes_and_extension_lists_read_well() {
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(wire::MAX_INPUT_BYTES), "32 MB");
    assert_eq!(format_size(50_541_363), "48.2 MB");
    assert_eq!(format_size(1536), "1.5 KB");
    assert_eq!(
        extension_list(&[Format::Text, Format::Markdown, Format::Pdf, Format::Docx]),
        ".txt, .md, .pdf, .docx"
    );
}

#[test]
fn args_default_and_validate() {
    let (formats, max) = validate_args(None).unwrap();
    assert_eq!(formats, wire::readable_formats().collect::<Vec<_>>());
    assert_eq!(max, DEFAULT_MAX_CHARS);

    let args = |formats: Option<Vec<Format>>, max_chars| {
        validate_args(Some(PickFileTextArgs { formats, max_chars }))
    };
    let key = |r: Result<(Vec<Format>, u32), AppError>| r.unwrap_err().key;
    assert_eq!(
        args(Some(vec![Format::Pdf, Format::Text]), Some(1)).unwrap(),
        (vec![Format::Pdf, Format::Text], 1)
    );
    assert_eq!(args(None, Some(1_000_000)).unwrap().1, 1_000_000);
    assert_eq!(
        key(args(Some(vec![]), None)),
        "errors.file_text.invalid_formats"
    );
    assert_eq!(
        key(args(Some(vec![Format::Pdf, Format::Pdf]), None)),
        "errors.file_text.invalid_formats"
    );
    assert_eq!(
        key(args(None, Some(0))),
        "errors.file_text.invalid_max_chars"
    );
    let e = args(None, Some(1_000_001)).unwrap_err();
    assert_eq!(e.params["max"], "1000000");
    assert!(matches!(e.kind, ErrorKind::InvalidInput));

    // Formats this build doesn't read yet are dropped; none left is refused.
    if !wire::READS_P1_FORMATS {
        assert_eq!(
            args(Some(vec![Format::Pptx, Format::Pdf]), None).unwrap().0,
            vec![Format::Pdf]
        );
        let e = args(Some(vec![Format::Html, Format::Rtf]), None).unwrap_err();
        assert_eq!(e.key, "errors.file_text.invalid_formats");
        assert_eq!(e.params["formats"], ".txt, .md, .pdf, .docx");
    }
}

#[test]
fn args_refuse_unknown_fields_and_formats_when_parsed() {
    let parse = |v: serde_json::Value| serde_json::from_value::<PickFileTextArgs>(v);
    let ok = parse(serde_json::json!({ "formats": ["pdf", "markdown"], "max_chars": 5 })).unwrap();
    assert_eq!(ok.formats, Some(vec![Format::Pdf, Format::Markdown]));
    assert_eq!(ok.max_chars, Some(5));
    assert_eq!(
        parse(serde_json::json!({})).unwrap(),
        PickFileTextArgs::default()
    );
    assert!(parse(serde_json::json!({ "maxChars": 5 })).is_err());
    assert!(parse(serde_json::json!({ "formats": ["exe"] })).is_err());
    assert!(parse(serde_json::json!({ "max_chars": -1 })).is_err());
}

#[test]
fn prepare_checks_the_id_the_grant_and_the_switch() {
    let config = config_with("mindmap", &["storage", "file_text"], None);
    let r = prepare(&config, "mindmap", "  Mind\u{202E}\n Map ", None).unwrap();
    assert_eq!(r.display_name, "Mind Map");
    assert_eq!(r.max_chars, DEFAULT_MAX_CHARS);
    assert_eq!(
        prepare(&config, "mindmap", " \u{200F} ", None)
            .unwrap()
            .display_name,
        "mindmap"
    );
    let long = "x".repeat(100);
    assert_eq!(
        prepare(&config, "mindmap", &long, None)
            .unwrap()
            .display_name
            .len(),
        64
    );

    let e = prepare(&config, "../evil", "Evil", None).unwrap_err();
    assert_eq!(e.key, "errors.extension.invalid_id");
    assert!(matches!(e.kind, ErrorKind::InvalidInput));

    for cfg in [
        Config::default(),
        config_with("mindmap", &["storage"], None),
    ] {
        let e = prepare(&cfg, "mindmap", "Mind Map", None).unwrap_err();
        assert_eq!(e.key, "errors.file_text.not_granted");
        assert_eq!(e.params["extension"], "Mind Map");
        assert!(matches!(e.kind, ErrorKind::NotPermitted));
    }
    let off = config_with("mindmap", &["file_text"], Some(false));
    let e = prepare(&off, "mindmap", "Mind Map", None).unwrap_err();
    assert_eq!(e.key, "errors.file_text.extension_disabled");
    let on = config_with("mindmap", &["FILE_TEXT"], Some(true));
    assert!(prepare(&on, "mindmap", "Mind Map", None).is_ok());

    // Arguments are checked after the grant.
    let bad = Some(PickFileTextArgs {
        formats: None,
        max_chars: Some(0),
    });
    assert_eq!(
        prepare(&Config::default(), "mindmap", "M", bad.clone())
            .unwrap_err()
            .key,
        "errors.file_text.not_granted"
    );
    assert_eq!(
        prepare(&config, "mindmap", "M", bad).unwrap_err().key,
        "errors.file_text.invalid_max_chars"
    );
}

#[test]
fn log_ids_never_echo_what_isnt_an_id() {
    assert_eq!(log_id("mindmap"), "mindmap");
    assert_eq!(log_id("../notes/secret.pdf"), "<invalid id>");
}

// --- The slot ---------------------------------------------------------------------------

#[test]
fn one_request_at_a_time_and_a_gap_between_dialogs() {
    let slot = Arc::new(Slot::default());
    let t0 = Instant::now();
    let a = slot.take("a", t0).unwrap();
    assert_eq!(slot.take("b", t0).unwrap_err(), Code::Busy);
    assert_eq!(slot.take("a", t0).unwrap_err(), Code::Busy);
    a.dialog_closed(t0);
    drop(a);

    assert_eq!(
        slot.take("a", t0 + Duration::from_secs(2)).unwrap_err(),
        Code::TooSoon
    );
    let b = slot.take("b", t0 + Duration::from_secs(2)).unwrap();
    drop(b);
    let a = slot.take("a", t0 + wire::DIALOG_GAP).unwrap();
    assert!(!a.cancelled());
    assert!(!slot.cancel("b"));
    assert!(slot.cancel("a"));
    assert!(a.cancelled());
    drop(a);
    assert!(!slot.cancel("a"), "nothing runs once the guard is gone");
}

#[test]
fn the_app_slot_cancels_nothing_when_idle() {
    assert!(!cancel("nobody-running"));
}

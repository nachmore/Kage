//! No log line holds the file name, its folder or its text (§5.15).

use super::flow::{extraction, temp_file};
use super::*;

mod capture {
    use std::sync::{Mutex, OnceLock};

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct Capture;

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            if record.target().contains("file_text") {
                LINES.lock().unwrap().push(record.args().to_string());
            }
        }
        fn flush(&self) {}
    }

    static CAPTURE: Capture = Capture;

    /// Installs the capturing logger once for this test binary. Nothing
    /// else in the unit tests installs a logger.
    pub fn install() -> bool {
        static INSTALLED: OnceLock<bool> = OnceLock::new();
        *INSTALLED.get_or_init(|| {
            let ok = log::set_logger(&CAPTURE).is_ok();
            log::set_max_level(log::LevelFilter::Trace);
            ok
        })
    }

    pub fn lines() -> Vec<String> {
        LINES.lock().unwrap().clone()
    }
}

#[test]
fn no_log_line_holds_the_file_name_or_its_folder() {
    assert!(capture::install(), "another logger is installed");
    let name = "Zebra-Plan-q7.txt";
    let (dir, path) = temp_file(name, b"zebra text");
    let folder = dir.path().to_string_lossy().into_owned();
    let req = request("logcheck", &[Format::Text, Format::Pdf]);
    let fakes = [
        Fake::Echo,
        Fake::Answer(extraction("password: hunter2-hunter2")),
        Fake::Fail(WorkerFailure::Refused(WorkerErrorCode::NotText)),
        Fake::Fail(WorkerFailure::Refused(WorkerErrorCode::WrongContent)),
        Fake::Fail(WorkerFailure::Timeout),
        Fake::Fail(WorkerFailure::Crashed("signal 6".into())),
    ];
    for fake in fakes {
        let events = Recorder::default();
        let _ = run_on(
            &Arc::default(),
            &FixedDialog::picks(&path),
            &fake,
            &events,
            &req,
        );
    }
    let lines = capture::lines();
    assert!(
        lines.iter().any(|l| l.contains("logcheck")),
        "the request logged nothing: {lines:?}"
    );
    for line in &lines {
        assert!(!line.contains("Zebra"), "file name in a log line: {line}");
        assert!(!line.contains(&folder), "folder in a log line: {line}");
        assert!(!line.contains("hunter2"), "file text in a log line: {line}");
    }
}

//! Tests for pure startup helpers extracted from main.rs.
//!
//! These are the bits of `fn main()` that don't depend on Tauri's runtime:
//! CLI flag parsing, session-id resume resolution, ACP mode dispatch, and
//! the WebView2 wait loop. Keeping them covered here means a refactor of
//! main.rs — splitting it into startup stages, extracting the setup
//! closure, etc. — can rely on these contracts.

use kage::acp_client::AcpConnectionMode;
use kage::config::AcpMode;
use kage::startup::{
    self, acp_mode_for, detect_capture_hotkey_subcommand, resolve_resume_session_id,
    wait_for_webview_release, CliFlags, WebviewWaitResult,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------------------
// CliFlags::parse
// ---------------------------------------------------------------------------

#[test]
fn cli_flags_default_when_no_matching_args() {
    let parsed = CliFlags::parse(&args(&["kage.exe"]));
    assert_eq!(parsed, CliFlags::default());
    assert!(!parsed.dev_mode && !parsed.debug_mode && !parsed.is_restart);
}

#[test]
fn cli_flags_accepts_slash_and_dash_variants() {
    let slash = CliFlags::parse(&args(&["kage.exe", "/dev", "/debug", "/restart"]));
    let dash = CliFlags::parse(&args(&["kage.exe", "--dev", "--debug", "--restart"]));
    assert!(slash.dev_mode && slash.debug_mode && slash.is_restart);
    assert_eq!(slash, dash);
}

#[test]
fn cli_flags_ignores_unknown_args() {
    let parsed = CliFlags::parse(&args(&[
        "kage.exe", "--dev", "/resume-session", "abc", "--weird-flag",
    ]));
    assert!(parsed.dev_mode);
    assert!(!parsed.debug_mode);
    assert!(!parsed.is_restart);
}

#[test]
fn cli_flags_each_flag_independent() {
    let only_debug = CliFlags::parse(&args(&["kage.exe", "--debug"]));
    assert!(only_debug.debug_mode && !only_debug.dev_mode && !only_debug.is_restart);
    let only_restart = CliFlags::parse(&args(&["kage.exe", "/restart"]));
    assert!(only_restart.is_restart && !only_restart.dev_mode && !only_restart.debug_mode);
}

// ---------------------------------------------------------------------------
// detect_capture_hotkey_subcommand
// ---------------------------------------------------------------------------

#[test]
fn capture_hotkey_absent_returns_none() {
    assert_eq!(detect_capture_hotkey_subcommand(&args(&["kage.exe"])), None);
    assert_eq!(
        detect_capture_hotkey_subcommand(&args(&["kage.exe", "--dev"])),
        None
    );
}

#[test]
fn capture_hotkey_parses_custom_timeout() {
    let a = args(&["kage.exe", "/capture-hotkey", "5000"]);
    assert_eq!(detect_capture_hotkey_subcommand(&a), Some(5000));
}

#[test]
fn capture_hotkey_defaults_timeout_when_missing() {
    let a = args(&["kage.exe", "/capture-hotkey"]);
    assert_eq!(detect_capture_hotkey_subcommand(&a), Some(10_000));
}

#[test]
fn capture_hotkey_defaults_timeout_when_unparseable() {
    let a = args(&["kage.exe", "/capture-hotkey", "not-a-number"]);
    assert_eq!(detect_capture_hotkey_subcommand(&a), Some(10_000));
}

// ---------------------------------------------------------------------------
// resolve_resume_session_id
// ---------------------------------------------------------------------------

#[test]
fn resume_session_cli_arg_wins_and_trims() {
    let tmp = tempdir_scoped();
    // No last-session.txt file, pure CLI arg path
    let a = args(&["kage.exe", "--resume-session", "  my-session-id  "]);
    assert_eq!(
        resolve_resume_session_id(&a, tmp.path()),
        Some("my-session-id".to_string())
    );
}

#[test]
fn resume_session_accepts_slash_variant() {
    let tmp = tempdir_scoped();
    let a = args(&["kage.exe", "/resume-session", "sess-42"]);
    assert_eq!(
        resolve_resume_session_id(&a, tmp.path()),
        Some("sess-42".to_string())
    );
}

#[test]
fn resume_session_falls_back_to_last_session_file() {
    let tmp = tempdir_scoped();
    let marker = tmp.path().join("last-session.txt");
    std::fs::write(&marker, "\nfile-session-id\n").unwrap();
    let got = resolve_resume_session_id(&args(&["kage.exe"]), tmp.path());
    assert_eq!(got, Some("file-session-id".to_string()));
    // File must be consumed (removed) so the session isn't resumed again
    // on the next launch.
    assert!(!marker.exists(), "last-session.txt should be deleted after read");
}

#[test]
fn resume_session_empty_file_is_treated_as_none() {
    let tmp = tempdir_scoped();
    let marker = tmp.path().join("last-session.txt");
    std::fs::write(&marker, "   \n  \t\n").unwrap();
    let got = resolve_resume_session_id(&args(&["kage.exe"]), tmp.path());
    assert_eq!(got, None);
    // File is still consumed so we don't keep reading empties.
    assert!(!marker.exists());
}

#[test]
fn resume_session_no_sources_returns_none() {
    let tmp = tempdir_scoped();
    let got = resolve_resume_session_id(&args(&["kage.exe"]), tmp.path());
    assert_eq!(got, None);
}

#[test]
fn resume_session_cli_arg_preempts_file_even_when_file_present() {
    let tmp = tempdir_scoped();
    let marker = tmp.path().join("last-session.txt");
    std::fs::write(&marker, "file-session-id\n").unwrap();
    let a = args(&["kage.exe", "--resume-session", "cli-session-id"]);
    let got = resolve_resume_session_id(&a, tmp.path());
    assert_eq!(got, Some("cli-session-id".to_string()));
    // CLI arg path should NOT consume the marker file; it remains for a
    // subsequent launch that might need it. The original code had this
    // behaviour and tests should lock it in.
    assert!(
        marker.exists(),
        "last-session.txt should be preserved when CLI arg supplied the id"
    );
}

#[test]
fn resume_session_cli_arg_missing_value_falls_through() {
    // "--resume-session" with no argument after it should NOT cause a
    // panic, and should fall through to the file lookup.
    let tmp = tempdir_scoped();
    let got = resolve_resume_session_id(&args(&["kage.exe", "--resume-session"]), tmp.path());
    assert_eq!(got, None);
}

// ---------------------------------------------------------------------------
// acp_mode_for
// ---------------------------------------------------------------------------

#[test]
fn acp_mode_local_maps_one_to_one() {
    let (mode, desc) = acp_mode_for(&AcpMode::Local {
        spawn_command: "kage-cli acp".to_string(),
    });
    match mode {
        AcpConnectionMode::Local { spawn_command } => {
            assert_eq!(spawn_command, "kage-cli acp");
        }
        _ => panic!("expected Local"),
    }
    assert!(desc.contains("Local"));
    assert!(desc.contains("kage-cli acp"));
}

#[test]
fn acp_mode_remote_carries_host_port_timeout_in_desc() {
    let (mode, desc) = acp_mode_for(&AcpMode::Remote {
        host: "192.0.2.10".to_string(),
        port: 9876,
        timeout_ms: 12_345,
    });
    match mode {
        AcpConnectionMode::Remote { host, port } => {
            assert_eq!(host, "192.0.2.10");
            assert_eq!(port, 9876);
        }
        _ => panic!("expected Remote"),
    }
    assert!(desc.contains("192.0.2.10"));
    assert!(desc.contains("9876"));
    // The transport mode doesn't carry the timeout, so the log is the
    // only surface where it's visible — lock that in.
    assert!(desc.contains("12345"));
}

// ---------------------------------------------------------------------------
// wait_for_webview_release
// ---------------------------------------------------------------------------

#[test]
fn webview_wait_returns_not_present_when_dir_missing() {
    let nonexistent = PathBuf::from(format!(
        "{}/kage-nonexistent-{}",
        std::env::temp_dir().display(),
        uuid::Uuid::new_v4()
    ));
    let out = wait_for_webview_release(
        &nonexistent,
        20,
        Duration::from_millis(500),
        |_| panic!("should never sleep when dir missing"),
    );
    assert_eq!(out, WebviewWaitResult::NotPresent);
}

#[test]
fn webview_wait_returns_released_on_first_attempt_when_writable() {
    let tmp = tempdir_scoped();
    let calls = Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let out = wait_for_webview_release(
        tmp.path(),
        20,
        Duration::from_millis(10),
        move |_| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
        },
    );
    match out {
        WebviewWaitResult::Released { waited_ms } => {
            // One sleep of 10ms before the first probe attempt.
            assert_eq!(waited_ms, 10);
        }
        other => panic!("expected Released, got {:?}", other),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn webview_wait_times_out_when_probe_always_fails() {
    // Point at a file path instead of a directory — File::create on a
    // path whose parent is a file fails with NotADirectory or similar,
    // reliably exercising the timeout branch without us having to race
    // file locks.
    let tmp = tempdir_scoped();
    let stub_file = tmp.path().join("looks-like-a-dir");
    std::fs::write(&stub_file, "").unwrap();

    let calls = Arc::new(AtomicU32::new(0));
    let calls_clone = calls.clone();
    let out = wait_for_webview_release(
        &stub_file, // exists but isn't a directory → File::create inside always fails
        3,
        Duration::from_millis(1),
        move |_| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
        },
    );
    match out {
        WebviewWaitResult::TimedOut { waited_ms } => {
            // 3 attempts × 1ms each.
            assert_eq!(waited_ms, 3);
        }
        other => panic!("expected TimedOut, got {:?}", other),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

// ---------------------------------------------------------------------------
// webview_user_data_dir
// ---------------------------------------------------------------------------

#[test]
fn webview_user_data_dir_is_under_kage_namespace() {
    // On every platform dirs::data_local_dir() is Some(...), so this
    // should always return Some and include /kage/EBWebView at the end.
    let dir = startup::webview_user_data_dir().expect("data_local_dir is unavailable on this host");
    let p = dir.to_string_lossy();
    assert!(p.contains("kage"), "path {} should include 'kage'", p);
    assert!(p.ends_with("EBWebView"), "path {} should end with EBWebView", p);
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Scoped temp directory that cleans itself up on drop. Avoids pulling
/// in the `tempfile` crate as a dev-dep just for these tests.
struct ScopedTempDir(PathBuf);

impl ScopedTempDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScopedTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tempdir_scoped() -> ScopedTempDir {
    let dir = std::env::temp_dir().join(format!("kage-startup-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    ScopedTempDir(dir)
}


// ---------------------------------------------------------------------------
// load_config_with_overrides
// ---------------------------------------------------------------------------

use kage::config::Config;

#[test]
fn load_config_uses_loaded_value_when_ok() {
    // Loader returns a custom config; the result should preserve its fields.
    let custom = Config { first_run_completed: true, ..Config::default() };

    let result = kage::startup::load_config_with_overrides(false, || Ok(custom.clone()));
    assert!(result.first_run_completed);
}

#[test]
fn load_config_falls_back_to_default_on_loader_error() {
    // When the loader fails we want to keep the app running. Verify we
    // get a sane default rather than a panic.
    let result = kage::startup::load_config_with_overrides(
        false,
        || anyhow::bail!("simulated load failure"),
    );
    // Default has first_run_completed = false.
    assert!(!result.first_run_completed);
    // And the version is the current schema version.
    assert_eq!(result.version, kage::config_migrations::CURRENT_VERSION);
}

#[test]
fn load_config_debug_flag_forces_debug_mode_on() {
    // If the persisted config has debug_mode = false but --debug was on
    // the CLI, the combined result should have debug_mode = true.
    let loaded = Config::default(); // debug_mode = false by default
    assert!(!loaded.debug_mode);
    let result = kage::startup::load_config_with_overrides(true, || Ok(loaded));
    assert!(result.debug_mode, "--debug CLI flag should force debug_mode on");
}

#[test]
fn load_config_debug_flag_off_does_not_clobber_persisted_true() {
    // The inverse: if the user saved debug_mode=true in their config,
    // running without --debug should NOT flip it back off.
    let loaded = Config { debug_mode: true, ..Config::default() };
    let result = kage::startup::load_config_with_overrides(false, || Ok(loaded));
    assert!(result.debug_mode, "persisted debug_mode=true must not be clobbered");
}

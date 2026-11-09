//! Reading bytes out of a user-installed extension's directory: locale
//! catalogs (`_locales/<lang>/messages.json`) and arbitrary provider/settings
//! files. Both validate the extension id and guard against `..` traversal.
//! Also the extension-data store, whose writes tell the extension's other
//! instances (`crate::extension_data_events`).

use super::*;
use crate::extension_data_events::{self, Change, ChangeOp, ChangeSource};

/// Load an extension's `_locales/<lang>/messages.json`. Falls back through
/// region-stripped variants ("zh-CN" → "zh") and finally to "en". Returns the
/// catalog as a JSON object so the host can hand it directly to the sandbox
/// runtime; an extension with no `_locales/` ships back an empty object,
/// which the runtime treats as "no translations, render keys verbatim".
///
/// Path-containment is validated identically to `read_extension_file` to
/// keep extensions from escaping their own directory via `..` segments in
/// the language code. The language argument is restricted to a small
/// alphabet (letters, digits, hyphens) for the same reason.
///
/// `staged` (install prompt only) reads from an upgrade parked for approval,
/// when there is one, since the live dir still holds the old version.
#[tauri::command]
pub async fn read_extension_locale(
    extension_id: String,
    kind: String,
    language: String,
    staged: Option<bool>,
) -> Result<serde_json::Value, AppError> {
    extensions::validate_extension_id(&extension_id).map_err(|e| {
        AppError::keyed(
            crate::error::ErrorKind::Internal,
            "errors.extension.invalid_id",
            &[("reason", &e.to_string())],
        )
    })?;

    if !language
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
        || language.is_empty()
        || language.len() > 16
    {
        return Err(AppError::keyed(
            crate::error::ErrorKind::Internal,
            "errors.extension.invalid_locale",
            &[("language", &language)],
        ));
    }

    let subdir = extensions::kind_to_subdir(&kind).map_err(|e| {
        AppError::keyed(
            crate::error::ErrorKind::Internal,
            "errors.extension.invalid_kind",
            &[("reason", &e.to_string())],
        )
    })?;
    let base = extensions::user_item_dir(subdir).map_err(|e| {
        AppError::keyed(
            crate::error::ErrorKind::Internal,
            "errors.extension.dir_unavailable",
            &[("reason", &e.to_string())],
        )
    })?;
    let parked = if staged.unwrap_or(false) {
        extensions::parked_upgrade_dir(&base, &extension_id)
    } else {
        None
    };
    let ext_root = parked.unwrap_or_else(|| base.join(&extension_id));
    let locales_dir = ext_root.join("_locales");

    // Try the requested language, then region-stripped form, then en. The
    // first hit wins; an entirely-absent _locales directory returns `{}`.
    let candidates: Vec<String> = {
        let mut out = vec![language.clone()];
        if let Some((stem, _)) = language.split_once('-') {
            if !out.contains(&stem.to_string()) {
                out.push(stem.to_string());
            }
        }
        if !out.iter().any(|c| c == "en") {
            out.push("en".to_string());
        }
        out
    };

    for cand in &candidates {
        let path = locales_dir.join(cand).join("messages.json");
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            AppError::keyed(
                crate::error::ErrorKind::Internal,
                "errors.extension.locale_read_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            AppError::keyed(
                crate::error::ErrorKind::Internal,
                "errors.extension.locale_parse_failed",
                &[("language", cand), ("reason", &e.to_string())],
            )
        })?;
        return Ok(value);
    }

    // No catalog at all — return an empty object so the runtime can still boot.
    Ok(serde_json::json!({}))
}

/// Read a file from a user-installed extension's directory.
/// Returns the file content as a string. Used by the frontend to dynamically
/// load search providers and settings modules from user-installed extensions.
#[tauri::command]
pub async fn read_extension_file(
    extension_id: String,
    kind: String,
    file_path: String,
) -> Result<String, AppError> {
    // Validate the extension id before it's spliced into any path. The
    // file_path containment check below is gated by `.exists()` and would
    // be skipped for a non-existent path; validating the id upfront makes
    // this fail closed on hostile ids regardless of which branch wins.
    extensions::validate_extension_id(&extension_id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;

    // Validate file_path to prevent directory traversal
    if file_path.contains("..") || file_path.contains('\\') || file_path.starts_with('/') {
        return Err("Invalid file path".into());
    }

    let subdir = extensions::kind_to_subdir(&kind).map_err(|e| format!("Invalid kind: {}", e))?;
    let base =
        extensions::user_item_dir(subdir).map_err(|e| format!("Failed to get directory: {}", e))?;
    let full_path = base.join(&extension_id).join(&file_path);

    // Verify the resolved path is within the extension directory
    let canonical_base = base.join(&extension_id);
    if full_path.exists() {
        let canonical = full_path
            .canonicalize()
            .map_err(|e| format!("Path error: {}", e))?;
        let canonical_parent = canonical_base
            .canonicalize()
            .map_err(|e| format!("Path error: {}", e))?;
        if !canonical.starts_with(&canonical_parent) {
            return Err("Path traversal detected".into());
        }
    }

    Ok(std::fs::read_to_string(&full_path).map_err(|e| format!("Failed to read file: {}", e))?)
}

// ---------------------------------------------------------------------------
// Generic extension data persistence
// ---------------------------------------------------------------------------
// Stores extension data as JSON files at:
//   <config_dir>/kage/extension-data/<extension_id>/<key>.json
//
// The path-resolution and migration logic lives in src/extensions.rs so it
// can be unit-tested without standing up a Tauri AppHandle. The host JS
// bridge force-injects extension_id from its own record before forwarding
// storage commands here, so a sandboxed caller can't spoof a different
// extension's identity.

/// Returns the root extension-data directory, creating it if needed.
fn extension_data_root() -> Result<std::path::PathBuf, String> {
    let dir = crate::paths::config_dir()
        .ok_or("No config directory")?
        .join("kage")
        .join("extension-data");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create extension-data dir: {}", e))?;
    }
    Ok(dir)
}

/// Resolve the on-disk path for a given (extension_id, key).
fn resolve_data_path(extension_id: &str, key: &str) -> Result<std::path::PathBuf, String> {
    let root = extension_data_root()?;
    extensions::resolve_extension_data_path(&root, extension_id, key).map_err(|e| format!("{}", e))
}

/// Delete everything an extension stored (its whole data dir). Called on
/// uninstall; a missing dir counts as success. When something was removed,
/// the extension's remaining instances get a `bulk` change (source
/// `uninstall`), though they are torn down anyway.
pub(super) fn purge_extension_data<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    extension_id: &str,
) -> Result<(), String> {
    if remove_extension_data_dir(extension_id)? {
        extension_data_events::record_bulk(app, extension_id, ChangeSource::Uninstall);
    }
    Ok(())
}

/// Remove an extension's data dir. `Ok(true)` when it existed.
fn remove_extension_data_dir(extension_id: &str) -> Result<bool, String> {
    extensions::validate_extension_id(extension_id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;
    let dir = extension_data_root()?.join(extension_id);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {
            info!("Removed stored data for '{}'", extension_id);
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!(
            "Failed to remove extension data for '{}': {}",
            extension_id, e
        )),
    }
}

/// Write one data file, atomically and owner-only, then name the change
/// for the coalescer. A failed write names nothing.
fn write_extension_data(
    extension_id: &str,
    key: &str,
    data: &str,
    window_label: &str,
) -> Result<Change, String> {
    let path = resolve_data_path(extension_id, key)?;
    crate::atomic_write::write(&path, data, crate::atomic_write::Perms::Private).map_err(|e| {
        format!(
            "Failed to save extension data '{}/{}': {}",
            extension_id, key, e
        )
    })?;
    Ok(Change::from_window(key, ChangeOp::Save, window_label))
}

/// Delete one data file, then name the change for the coalescer. A key
/// with no file is no change (`Ok(None)`); a failed delete names nothing.
fn remove_extension_data(
    extension_id: &str,
    key: &str,
    window_label: &str,
) -> Result<Option<Change>, String> {
    let path = resolve_data_path(extension_id, key)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Some(Change::from_window(
            key,
            ChangeOp::Delete,
            window_label,
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "Failed to delete extension data '{}/{}': {}",
            extension_id, key, e
        )),
    }
}

/// Save arbitrary JSON data for an extension.
/// Stored at: <config_dir>/kage/extension-data/<extension_id>/<key>.json
///
/// Atomic and owner-only (`crate::atomic_write`): extensions keep API keys
/// here, and a write torn by a crash or the updater's exit would leave a
/// file the extension can never parse again.
///
/// Once the file is in place, the extension's instances in every window
/// hear of it (`extension_data_changed`, EXTENSION_VIEWS §10). Tauri
/// injects `app` and `window`; callers pass only the first three.
#[tauri::command]
pub async fn save_extension_data<R: tauri::Runtime>(
    extension_id: String,
    key: String,
    data: String,
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,
) -> Result<(), AppError> {
    let change = write_extension_data(&extension_id, &key, &data, window.label())?;
    extension_data_events::record(&app, &extension_id, change);
    Ok(())
}

/// Load JSON data for an extension. Returns null if the file doesn't exist.
#[tauri::command]
pub async fn load_extension_data(
    extension_id: String,
    key: String,
) -> Result<Option<String>, AppError> {
    let path = resolve_data_path(&extension_id, &key)?;
    match std::fs::read_to_string(&path) {
        Ok(data) => Ok(Some(data)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "Failed to load extension data '{}/{}': {}",
            extension_id, key, e
        ))?,
    }
}

/// Delete extension data file. Deleting a key that has no file is a no-op
/// and tells no one; a real delete reaches the extension's instances in
/// every window, like a save.
#[tauri::command]
pub async fn delete_extension_data<R: tauri::Runtime>(
    extension_id: String,
    key: String,
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,
) -> Result<(), AppError> {
    if let Some(change) = remove_extension_data(&extension_id, &key, window.label())? {
        extension_data_events::record(&app, &extension_id, change);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn save(ext: &str, key: &str, data: &str) {
        let change = write_extension_data(ext, key, data, "floating").unwrap();
        assert_eq!(change, Change::from_window(key, ChangeOp::Save, "floating"));
    }

    fn load(ext: &str, key: &str) -> Option<String> {
        block_on(load_extension_data(ext.into(), key.into())).unwrap()
    }

    /// The extension-data root is shared with config_export's tests, whose
    /// backup round trip reads and rewrites all of it.
    fn data_root_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::config_export::EXTENSION_DATA_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn interrupted_write_never_reaches_the_reader() {
        let _root = data_root_lock();
        // crate::paths isolates unit tests to a scratch dir on first lookup.
        let _ = crate::paths::config_dir();
        assert!(crate::paths::is_isolated());
        let ext = "atomic-test-interrupted";
        let path = resolve_data_path(ext, "settings").unwrap();
        // State an earlier run left behind: the last complete save, plus
        // the temp of a save that a crash, force quit, or the updater's
        // exit cut off half-way.
        std::fs::write(&path, r#"{"apiKey":"sk-123"}"#).unwrap();
        let stale = path.with_file_name(".settings.json.kage-tmp-0123456789abcdef");
        std::fs::write(&stale, r#"{"apiK"#).unwrap();
        // Old enough that no live writer can still own it.
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();
        // A user's file that merely looks like a temp is never touched.
        let lookalike = path.with_file_name("settings.json.tmp.1.0");
        std::fs::write(&lookalike, "mine").unwrap();

        assert_eq!(
            load(ext, "settings").as_deref(),
            Some(r#"{"apiKey":"sk-123"}"#)
        );

        save(ext, "settings", r#"{"apiKey":"sk-456"}"#);
        assert_eq!(
            load(ext, "settings").as_deref(),
            Some(r#"{"apiKey":"sk-456"}"#)
        );
        // The first save sweeps the stale temp up. Not on Windows, which
        // doesn't sweep (see atomic_write::remove_stale_temps).
        assert_eq!(stale.exists(), cfg!(not(unix)), "stale temp after a save");
        assert_eq!(std::fs::read_to_string(&lookalike).unwrap(), "mine");
        std::fs::remove_file(&lookalike).unwrap();
        #[cfg(not(unix))]
        std::fs::remove_file(&stale).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(leftovers, vec!["settings.json".to_string()]);
    }

    #[test]
    fn concurrent_saves_always_leave_a_complete_file() {
        let _root = data_root_lock();
        let ext = "atomic-test-concurrent";
        let big_a = format!(r#"{{"v":"{}"}}"#, "a".repeat(64 * 1024));
        let big_b = format!(r#"{{"v":"{}"}}"#, "b".repeat(64 * 1024));
        save(ext, "blob", &big_a);
        let path = resolve_data_path(ext, "blob").unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let stop = stop.clone();
            let (path, a, b) = (path.clone(), big_a.clone(), big_b.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let got = std::fs::read_to_string(&path).unwrap();
                    assert!(
                        got == a || got == b,
                        "reader saw a torn file ({} bytes)",
                        got.len()
                    );
                }
            })
        };
        for i in 0..40 {
            save(ext, "blob", if i % 2 == 0 { &big_b } else { &big_a });
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        reader.join().unwrap();
    }

    #[test]
    fn a_change_is_named_only_after_a_successful_write() {
        let _root = data_root_lock();
        let ext = "events-test-writes";
        let change =
            write_extension_data(ext, "mm-meta", "{}", "ext-view:mindmap:explorer").expect("save");
        assert_eq!(
            change,
            Change::from_window("mm-meta", ChangeOp::Save, "ext-view:mindmap:explorer")
        );
        // The change is named once the data is readable.
        assert_eq!(load(ext, "mm-meta").as_deref(), Some("{}"));

        // A refused key or id writes nothing and names nothing.
        assert!(write_extension_data(ext, "../escape", "{}", "floating").is_err());
        assert!(write_extension_data("Bad Id", "k", "{}", "floating").is_err());
        assert!(remove_extension_data(ext, "../escape", "floating").is_err());

        // A delete is a change only when there was a file.
        assert_eq!(
            remove_extension_data(ext, "mm-meta", "main").unwrap(),
            Some(Change::from_window("mm-meta", ChangeOp::Delete, "main"))
        );
        assert_eq!(load(ext, "mm-meta"), None);
        assert_eq!(remove_extension_data(ext, "mm-meta", "main").unwrap(), None);
    }

    #[test]
    fn purging_reports_whether_there_was_data() {
        let _root = data_root_lock();
        let ext = "events-test-purge";
        save(ext, "a", "{}");
        let dir = extension_data_root().unwrap().join(ext);
        assert!(dir.join("a.json").exists());
        assert!(remove_extension_data_dir(ext).unwrap());
        assert!(!dir.exists());
        assert!(!remove_extension_data_dir(ext).unwrap());
        assert!(remove_extension_data_dir("../x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn extension_data_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let _root = data_root_lock();
        let ext = "atomic-test-perms";
        save(ext, "secret", "{}");
        let path = resolve_data_path(ext, "secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

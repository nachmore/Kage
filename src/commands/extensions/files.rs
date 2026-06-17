//! Reading bytes out of a user-installed extension's directory: locale
//! catalogs (`_locales/<lang>/messages.json`) and arbitrary provider/settings
//! files. Both validate the extension id and guard against `..` traversal.

use super::*;

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
#[tauri::command]
pub async fn read_extension_locale(
    extension_id: String,
    kind: String,
    language: String,
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
    let ext_root = base.join(&extension_id);
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
    let dir = dirs::config_dir()
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

/// Save arbitrary JSON data for an extension.
/// Stored at: <config_dir>/kage/extension-data/<extension_id>/<key>.json
#[tauri::command]
pub async fn save_extension_data(
    extension_id: String,
    key: String,
    data: String,
) -> Result<(), AppError> {
    let path = resolve_data_path(&extension_id, &key)?;
    std::fs::write(&path, &data).map_err(|e| {
        format!(
            "Failed to save extension data '{}/{}': {}",
            extension_id, key, e
        )
    })?;
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

/// Delete extension data file.
#[tauri::command]
pub async fn delete_extension_data(extension_id: String, key: String) -> Result<(), AppError> {
    let path = resolve_data_path(&extension_id, &key)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "Failed to delete extension data '{}/{}': {}",
            extension_id, key, e
        ))?,
    }
}

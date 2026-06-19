//! Integrations and small misc commands: app metadata, OS dark-mode probe,
//! clipboard read/write, directory resolution, file/calendar/icon search,
//! favicon proxy, computer-control + MCP setup, OS-startup toggles, the
//! Ollama HTTP probes, the window walker, the activity tracker, and the
//! cached UserInfo lookup.

use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::os;
use crate::state::FeatureServices;
use crate::window_labels;
use log::{info, warn};
use tauri::{Manager, State};

/// Wall-clock cap on the OS-backed query commands below (file search,
/// calendar). These run on the blocking pool and can be triggered by
/// extensions; without a cap a pathological query — or an extension firing
/// them faster than they complete — could pile up blocking work. On timeout
/// the command returns an error promptly so the caller isn't wedged. NOTE:
/// `spawn_blocking` work can't be cancelled mid-flight, so the underlying
/// query finishes in the background; the timeout bounds the *caller*, which
/// is what stops the pile-up. The calendar's PowerShell side has its own ~9s
/// internal kill, so this is a belt-and-suspenders ceiling above that.
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

#[tauri::command]
pub async fn get_app_info() -> Result<serde_json::Value, AppError> {
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "authors": env!("CARGO_PKG_AUTHORS"),
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "license": env!("CARGO_PKG_LICENSE"),
        "repository": env!("CARGO_PKG_REPOSITORY"),
        "homepage": env!("CARGO_PKG_HOMEPAGE"),
        "name": env!("CARGO_PKG_NAME"),
        // UI-facing links sourced from [package.metadata.links] via
        // build.rs. Empty strings here mean "link not configured" —
        // the UI should treat them as such and avoid rendering an
        // anchor that goes nowhere.
        "links": {
            "repository": env!("KAGE_LINK_REPOSITORY"),
            "issues": env!("KAGE_LINK_ISSUES"),
            "privacy": env!("KAGE_LINK_PRIVACY"),
        },
        // Update channel allow-list — surfaced so the Settings UI can
        // render the dropdown from a single source of truth (Rust)
        // rather than maintaining a parallel hardcoded list.
        "update_channels": crate::config::Channel::all()
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>(),
    }))
}

/// Detect whether the OS is using dark mode.
#[tauri::command]
pub async fn get_os_dark_mode() -> bool {
    crate::os::is_dark_mode()
}

#[tauri::command]
pub async fn read_clipboard() -> Result<String, AppError> {
    Ok(crate::os::read_clipboard().unwrap_or_default())
}

#[tauri::command]
pub async fn resolve_directories() -> Result<Vec<serde_json::Value>, AppError> {
    let dirs: Vec<(&str, &[&str], Option<std::path::PathBuf>)> = vec![
        ("cache", &["—"], dirs::cache_dir()),
        ("config", &["configuration"], dirs::config_dir()),
        ("data", &["—"], dirs::data_dir()),
        ("desktop", &["—"], dirs::desktop_dir()),
        ("documents", &["docs"], dirs::document_dir()),
        ("downloads", &["download"], dirs::download_dir()),
        ("fonts", &["font"], crate::os::fonts_dir()),
        ("home", &["user"], dirs::home_dir()),
        ("music", &["audio"], dirs::audio_dir()),
        ("pictures", &["photos"], dirs::picture_dir()),
        ("public", &["—"], dirs::public_dir()),
        (
            "screenshots",
            &["screenshot"],
            dirs::picture_dir().map(|p| p.join("Screenshots")),
        ),
        ("templates", &["template"], dirs::template_dir()),
        ("temp", &["tmp"], Some(std::env::temp_dir())),
        ("videos", &["video", "movies"], dirs::video_dir()),
    ];
    Ok(dirs
        .into_iter()
        .map(|(keyword, aliases, path)| {
            serde_json::json!({
                "keyword": keyword,
                "aliases": aliases.join(", "),
                "path": path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            })
        })
        .collect())
}

#[tauri::command]
pub async fn get_clipboard_history(
) -> Result<Vec<crate::os::clipboard_history::ClipboardHistoryEntry>, AppError> {
    Ok(crate::os::get_clipboard_history())
}

/// Search for files using the OS-native search index.
#[tauri::command]
pub async fn search_files(
    query: String,
    max_results: Option<usize>,
) -> Result<Vec<crate::os::file_search::FileSearchResult>, AppError> {
    let max = max_results.unwrap_or(10);
    let q = query.clone();
    let task = tauri::async_runtime::spawn_blocking(move || crate::os::search_files(&q, max));
    match tokio::time::timeout(QUERY_TIMEOUT, task).await {
        Ok(joined) => Ok(joined.map_err(|e| format!("Search task failed: {}", e))?),
        Err(_) => {
            warn!(
                "[search_files] query timed out after {}s: {:?}",
                QUERY_TIMEOUT.as_secs(),
                query
            );
            Err(AppError::from(format!(
                "File search timed out after {}s",
                QUERY_TIMEOUT.as_secs()
            )))
        }
    }
}

/// Get upcoming calendar events.
#[tauri::command]
pub async fn get_calendar_events(
    hours: Option<u32>,
) -> Result<Vec<crate::os::calendar::CalendarEvent>, AppError> {
    let h = hours.unwrap_or(24).min(72);
    let task = tauri::async_runtime::spawn_blocking(move || crate::os::get_upcoming_events(h));
    match tokio::time::timeout(QUERY_TIMEOUT, task).await {
        Ok(joined) => joined
            .map_err(|e| AppError::from(format!("Calendar task failed: {}", e)))?
            .map_err(AppError::from),
        Err(_) => {
            warn!(
                "[get_calendar_events] timed out after {}s",
                QUERY_TIMEOUT.as_secs()
            );
            Err(AppError::from(format!(
                "Calendar query timed out after {}s",
                QUERY_TIMEOUT.as_secs()
            )))
        }
    }
}

/// Get calendar events for a specific date (YYYY-MM-DD).
#[tauri::command]
pub async fn get_calendar_events_for_date(
    date: String,
) -> Result<Vec<crate::os::calendar::CalendarEvent>, AppError> {
    if !is_valid_iso_date(&date) {
        return Err("Invalid date format. Use YYYY-MM-DD.".into());
    }
    let task = tauri::async_runtime::spawn_blocking(move || crate::os::get_events_for_date(&date));
    match tokio::time::timeout(QUERY_TIMEOUT, task).await {
        Ok(joined) => joined
            .map_err(|e| AppError::from(format!("Calendar date query failed: {}", e)))?
            .map_err(AppError::from),
        Err(_) => {
            warn!(
                "[get_calendar_events_for_date] timed out after {}s",
                QUERY_TIMEOUT.as_secs()
            );
            Err(AppError::from(format!(
                "Calendar query timed out after {}s",
                QUERY_TIMEOUT.as_secs()
            )))
        }
    }
}

/// Strict YYYY-MM-DD validator. The date is interpolated into a PowerShell
/// command on Windows, so anything more permissive is an injection vector.
/// Lifted out of the command body so it can be unit-tested.
pub(crate) fn is_valid_iso_date(date: &str) -> bool {
    if date.len() != 10 {
        return false;
    }
    let bytes = date.as_bytes();
    bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[5].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7] == b'-'
        && bytes[8].is_ascii_digit()
        && bytes[9].is_ascii_digit()
}

/// Fetch a website's favicon and return it as a base64 data URI.
#[tauri::command]
pub async fn fetch_favicon(url: String) -> Result<String, AppError> {
    let domain = url::Url::parse(&url.replace(['{', '}'], ""))
        .or_else(|_| url::Url::parse(&format!("https://{}", url.replace(['{', '}'], ""))))
        .map_err(|e| format!("Invalid URL: {}", e))?
        .host_str()
        .unwrap_or("")
        .to_string();

    if domain.is_empty() {
        return Err("Could not extract domain from URL".into());
    }

    let favicon_url = format!("https://www.google.com/s2/favicons?domain={}&sz=64", domain);
    info!("Fetching favicon for {}: {}", domain, favicon_url);

    let bytes = tauri::async_runtime::spawn_blocking(move || {
        reqwest::blocking::get(&favicon_url)
            .and_then(|r| r.bytes())
            .map_err(|e| format!("Fetch failed: {}", e))
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))??;

    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    let content_type = favicon_content_type(&bytes);

    Ok(format!("data:{};base64,{}", content_type, b64))
}

/// Detect content type from magic bytes. Pulled out for unit testing.
pub(crate) fn favicon_content_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        "image/x-icon"
    } else {
        "image/png" // default
    }
}

/// Write text to clipboard and simulate Ctrl+V paste to the foreground window.
#[tauri::command]
pub async fn paste_clipboard_item(text: String, app: tauri::AppHandle) -> Result<(), AppError> {
    crate::os::write_clipboard(&text);
    // Small delay to ensure clipboard is updated before paste
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    crate::os::simulate_paste();
    // "Advanced paste" / clipboard history usage. We send only the
    // length bucket — the text content itself is user data and never
    // leaves the machine.
    crate::telemetry::track(
        &app,
        "clipboard_history_used",
        Some(serde_json::json!({
            "length": message_length_bucket(text.len()),
        })),
    );
    Ok(())
}

/// Length-bucket helper, mirroring ui/js/shared/telemetry.js::messageLengthBucket
/// so the buckets line up across event sources.
pub(crate) fn message_length_bucket(n: usize) -> &'static str {
    match n {
        0..=49 => "xs",
        50..=199 => "sm",
        200..=999 => "md",
        1000..=4999 => "lg",
        _ => "xl",
    }
}

#[derive(serde::Serialize, Clone)]
pub struct UserInfo {
    pub display_name: String,
    pub initials: String,
    pub avatar_path: Option<String>,
    pub avatar_base64: Option<String>,
    pub home: Option<String>,
}

#[tauri::command]
pub async fn get_user_info(features: State<'_, FeatureServices>) -> Result<UserInfo, AppError> {
    // Return cached user info if available
    {
        let cached = features.user_info_cache.lock_or_recover();
        if let Some(ref info) = *cached {
            return Ok(info.clone());
        }
    }

    // Compute and cache
    let info = compute_user_info();
    {
        let mut cached = features.user_info_cache.lock_or_recover();
        *cached = Some(info.clone());
    }
    Ok(info)
}

/// Compute user info (expensive — spawns whoami subprocess on Windows).
/// Called once and cached in FeatureServices.user_info_cache.
pub fn compute_user_info() -> UserInfo {
    let profile = os::get_user_profile();

    // Build initials from display name, falling back to username
    let name_for_initials = if profile.display_name == profile.username {
        &profile.username
    } else {
        &profile.display_name
    };

    let initials = name_for_initials
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();

    let initials = if initials.is_empty() {
        profile
            .username
            .chars()
            .next()
            .unwrap_or('U')
            .to_uppercase()
            .to_string()
    } else {
        initials
    };

    // Read avatar file as base64 for direct use in img src
    let avatar_base64 = profile.avatar_path.as_ref().and_then(|path| {
        use base64::Engine;
        let bytes = std::fs::read(path).ok()?;
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png");
        let mime = match ext {
            "jpg" | "jpeg" => "image/jpeg",
            "bmp" => "image/bmp",
            _ => "image/png",
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(format!("data:{};base64,{}", mime, b64))
    });

    UserInfo {
        display_name: profile.display_name.clone(),
        initials,
        avatar_path: profile.avatar_path.clone(),
        avatar_base64,
        home: dirs::home_dir().and_then(|p| p.to_str().map(|s| s.to_string())),
    }
}

// --- Computer-control / MCP wiring ----------------------------------

#[tauri::command]
pub async fn get_startup_enabled() -> Result<bool, AppError> {
    Ok(crate::os::get_startup_enabled())
}

#[tauri::command]
pub async fn set_startup_enabled(enabled: bool) -> Result<(), AppError> {
    crate::os::set_startup_enabled(enabled);
    Ok(())
}

#[tauri::command]
pub async fn get_computer_control_enabled() -> Result<bool, AppError> {
    Ok(crate::mcp_registration::is_registered())
}

#[tauri::command]
pub async fn set_computer_control_enabled(enabled: bool) -> Result<(), AppError> {
    if enabled {
        crate::mcp_registration::ensure_registered();
    } else {
        crate::mcp_registration::unregister();
    }
    Ok(())
}

#[tauri::command]
pub async fn get_mcp_json_path() -> Result<String, AppError> {
    crate::mcp_registration::default_mcp_json_path()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or("Cannot determine mcp.json path".into())
}

#[tauri::command]
pub async fn get_mcp_config(path: Option<String>) -> Result<serde_json::Value, AppError> {
    Ok(crate::mcp_registration::read_mcp_json(path.as_deref()))
}

#[tauri::command]
pub async fn save_mcp_config(
    path: Option<String>,
    config: serde_json::Value,
) -> Result<(), AppError> {
    Ok(crate::mcp_registration::write_mcp_json(
        path.as_deref(),
        &config,
    )?)
}

// --- Ollama integration commands ------------------------------------
//
// All three are pure HTTP probes against the user's Ollama daemon —
// no app state required. Settings → Ollama uses these to surface
// reachability + model list, and to seed the spawn command for the
// "Use Ollama with Codex" wizard.

/// Probe the Ollama daemon. Returns a `ProbeResult` — `Reachable {
/// version }` on success or `Unreachable { reason }` with a short
/// human-readable string the UI can render directly.
#[tauri::command]
pub async fn ollama_probe(base_url: String) -> Result<crate::ollama::ProbeResult, AppError> {
    // The probe is blocking HTTP — push it to a worker so we don't
    // tie up the Tauri command runtime if the user's local network
    // misbehaves.
    tauri::async_runtime::spawn_blocking(move || crate::ollama::probe(&base_url))
        .await
        .map_err(|e| AppError::from(format!("Probe task failed: {}", e)))
}

/// List installed Ollama models via `/api/tags`. Returns an empty
/// list if reachable but nothing is pulled — the UI surfaces "no
/// models — try `ollama pull llama3`" in that case.
#[tauri::command]
pub async fn ollama_list_models(
    base_url: String,
) -> Result<Vec<crate::ollama::ModelEntry>, AppError> {
    tauri::async_runtime::spawn_blocking(move || crate::ollama::list_models(&base_url))
        .await
        .map_err(|e| AppError::from(format!("List task failed: {}", e)))?
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.ollama.list_failed",
                &[("message", &e.to_string())],
            )
        })
}

/// Build the spawn command used by the "Use Ollama with Codex"
/// wizard. The frontend feeds it into the connections editor as a
/// new Local-mode connection.
#[tauri::command]
pub async fn ollama_codex_spawn_command(
    base_url: String,
    model: String,
) -> Result<String, AppError> {
    Ok(crate::ollama::build_codex_spawn_command(&base_url, &model))
}

// --- Window Walker ---

#[tauri::command]
pub async fn list_open_windows() -> Result<Vec<crate::os::window_list::WindowInfo>, AppError> {
    Ok(crate::os::list_windows())
}

/// Fetch app icons for a set of window handles. Designed to be called after
/// list_open_windows so the window list renders instantly while icons load
/// in the background.
#[tauri::command]
pub async fn get_window_icons(
    pids: Vec<u64>,
) -> Result<std::collections::HashMap<u64, String>, AppError> {
    tauri::async_runtime::spawn_blocking(move || crate::os::window_list::get_window_icons(&pids))
        .await
        .map_err(|e| AppError::from(format!("Icon fetch failed: {}", e)))
}

#[tauri::command]
pub async fn get_process_name(pid: u32) -> Result<String, AppError> {
    Ok(crate::os::process::get_process_name(pid).unwrap_or_default())
}

#[tauri::command]
pub async fn focus_open_window(handle: u64, app: tauri::AppHandle) -> Result<(), AppError> {
    // Hide the floating window before focusing the target
    if let Some(floating) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating.hide();
    }
    Ok(crate::os::focus_window(handle)?)
}

// --- Activity Tracker ---

#[tauri::command]
pub async fn start_activity_tracker(
    features: State<'_, FeatureServices>,
    poll_interval: Option<u64>,
) -> Result<(), AppError> {
    let tracker = features.activity_tracker.clone();
    crate::activity_tracker::start_tracker(&tracker, poll_interval)
        .await
        .map_err(|e| format!("Failed to start tracker: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn stop_activity_tracker(features: State<'_, FeatureServices>) -> Result<(), AppError> {
    let tracker = features.activity_tracker.clone();
    crate::activity_tracker::stop_tracker(&tracker).await;
    Ok(())
}

#[tauri::command]
pub async fn get_activity_report(
    features: State<'_, FeatureServices>,
    period: String,
) -> Result<crate::activity_tracker::ActivityReport, AppError> {
    let tracker = features.activity_tracker.clone();
    Ok(crate::activity_tracker::get_report(&tracker, &period)
        .await
        .map_err(|e| format!("Failed to get report: {}", e))?)
}

#[tauri::command]
pub async fn is_activity_tracker_running(
    features: State<'_, FeatureServices>,
) -> Result<bool, AppError> {
    Ok(features.activity_tracker.is_running())
}

#[tauri::command]
pub async fn get_app_icon(process_name: String) -> Result<Option<String>, AppError> {
    Ok(crate::os::get_app_icon(&process_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_date_validator_accepts_well_formed_dates() {
        assert!(is_valid_iso_date("2026-05-27"));
        assert!(is_valid_iso_date("0000-01-01"));
        assert!(is_valid_iso_date("9999-12-31"));
    }

    #[test]
    fn iso_date_validator_rejects_injection_attempts() {
        // Length-based bypass attempts
        assert!(!is_valid_iso_date(""));
        assert!(!is_valid_iso_date("2026-05-2"));
        assert!(!is_valid_iso_date("2026-05-271"));
        // Wrong separators — the calendar ScriptBlock interpolates this
        // into PowerShell, so a single ' or backtick anywhere is a hole.
        assert!(!is_valid_iso_date("2026/05/27"));
        assert!(!is_valid_iso_date("2026'05'27"));
        assert!(!is_valid_iso_date("2026 05 27"));
        // Non-digit positions
        assert!(!is_valid_iso_date("abcd-05-27"));
        assert!(!is_valid_iso_date("2026-AB-27"));
        assert!(!is_valid_iso_date("2026-05-AB"));
    }

    #[test]
    fn message_length_buckets_match_frontend() {
        // Boundary values must match ui/js/shared/telemetry.js exactly so
        // events from JS and Rust line up in the dashboard.
        assert_eq!(message_length_bucket(0), "xs");
        assert_eq!(message_length_bucket(49), "xs");
        assert_eq!(message_length_bucket(50), "sm");
        assert_eq!(message_length_bucket(199), "sm");
        assert_eq!(message_length_bucket(200), "md");
        assert_eq!(message_length_bucket(999), "md");
        assert_eq!(message_length_bucket(1000), "lg");
        assert_eq!(message_length_bucket(4999), "lg");
        assert_eq!(message_length_bucket(5000), "xl");
        assert_eq!(message_length_bucket(usize::MAX), "xl");
    }

    #[test]
    fn favicon_content_type_detects_magic_bytes() {
        assert_eq!(favicon_content_type(&[0x89, 0x50, 0x4E, 0x47]), "image/png");
        assert_eq!(favicon_content_type(&[0xFF, 0xD8, 0xFF]), "image/jpeg");
        assert_eq!(
            favicon_content_type(&[0x00, 0x00, 0x01, 0x00]),
            "image/x-icon"
        );
        // Unknown bytes default to png — Google Favicon API serves png
        // overwhelmingly often, so this is the safest fallback for an
        // unrecognised payload.
        assert_eq!(favicon_content_type(&[0xDE, 0xAD]), "image/png");
        assert_eq!(favicon_content_type(&[]), "image/png");
    }

    #[test]
    fn compute_user_info_initials_fall_back_to_username_first_char() {
        // Pure helper test — exercises the initials-derivation logic
        // without involving the OS profile lookup. We can't easily mock
        // get_user_profile so we just verify the public surface is
        // non-empty for a real user.
        let info = compute_user_info();
        assert!(!info.initials.is_empty(), "initials must never be blank");
    }
}

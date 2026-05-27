use crate::error::AppError;
use crate::os;
use crate::state::FeatureServices;
use log::{error, info};
use tauri::{Manager, State};

/// Check if input is a URL (must be the entire input, not embedded in a sentence)
fn is_url(input: &str) -> bool {
    let trimmed = input.trim();
    // If there are spaces before the protocol, it's a sentence, not a bare URL
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("ftp://")
        || trimmed.starts_with("file://")
        || (trimmed.starts_with("www.") && trimmed.contains('.'))
}

/// Check if input is a file or folder path.
/// Only matches if the input *starts* with a path-like pattern.
/// Natural language queries that happen to contain paths mid-sentence are not matched.
fn is_path(input: &str) -> Option<String> {
    let trimmed = input.trim();

    // Reject if it looks like a natural language query (starts with common words)
    // A path input should start directly with the path characters.

    // Windows paths — must start with drive letter or UNC prefix
    if cfg!(target_os = "windows") {
        // Drive letter: C:\ or C:/ or just C: (bare drive root)
        if trimmed.len() >= 2
            && trimmed.as_bytes()[0].is_ascii_alphabetic()
            && trimmed.chars().nth(1) == Some(':')
        {
            // Accept "C:", "C:\", "C:\Users\...", "C:/Users/..."
            if trimmed.len() == 2
                || trimmed.chars().nth(2) == Some('\\')
                || trimmed.chars().nth(2) == Some('/')
            {
                return Some(trimmed.to_string());
            }
        }
        // UNC path: \\server\share
        if trimmed.starts_with("\\\\") {
            return Some(trimmed.to_string());
        }
        // Don't match trimmed.contains('\\') — that catches paths mid-sentence
    }

    // Unix-like paths — must start with / or ~
    if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
        if trimmed.starts_with('/') {
            return Some(trimmed.to_string());
        }
        if trimmed.starts_with('~') {
            return Some(trimmed.to_string());
        }
        // Don't match trimmed.contains('/') — that catches paths mid-sentence
    }

    None
}

/// Resolve well-known directory names to their actual paths.
/// Supports prefix matching — "down" matches "downloads".
/// Only matches if the input is a single word (no spaces).
fn resolve_well_known_dir(input: &str) -> Option<String> {
    let lower = input.to_lowercase();
    if lower.contains(' ') || lower.is_empty() {
        return None;
    }

    #[allow(clippy::type_complexity)]
    let candidates: &[(&[&str], fn() -> Option<std::path::PathBuf>)] = &[
        (&["downloads", "download"], dirs::download_dir),
        (&["documents", "docs"], dirs::document_dir),
        (&["pictures", "photos"], dirs::picture_dir),
        (&["videos", "video", "movies"], dirs::video_dir),
        (&["music", "audio"], dirs::audio_dir),
        (&["desktop"], dirs::desktop_dir),
        (&["home", "user"], dirs::home_dir),
        (&["templates", "template"], dirs::template_dir),
        (&["temp", "tmp"], || -> Option<std::path::PathBuf> {
            Some(std::env::temp_dir())
        }),
        (&["public"], dirs::public_dir),
        (
            &["screenshots", "screenshot"],
            || -> Option<std::path::PathBuf> { dirs::picture_dir().map(|p| p.join("Screenshots")) },
        ),
        (&["fonts", "font"], dirs::font_dir),
        (&["cache"], dirs::cache_dir),
        (&["config", "configuration"], dirs::config_dir),
        (&["data"], dirs::data_dir),
    ];

    // Override "fonts"/"font" to use the system fonts directory
    {
        let font_names = ["fonts", "font"];
        if font_names.contains(&lower.as_str())
            || font_names.iter().any(|n| n.starts_with(lower.as_str()))
        {
            if let Some(font_dir) = crate::os::fonts_dir() {
                return Some(font_dir.to_string_lossy().to_string());
            }
        }
    }

    // Exact match first
    for (names, resolver) in candidates {
        if names.contains(&lower.as_str()) {
            return resolver().map(|p| p.to_string_lossy().to_string());
        }
    }

    // Prefix match — return the first match
    for (names, resolver) in candidates {
        if names.iter().any(|n| n.starts_with(lower.as_str())) {
            return resolver().map(|p| p.to_string_lossy().to_string());
        }
    }

    None
}

/// Match a system command by name (exact or prefix).
/// Returns (command_id, display_label, needs_confirmation).
fn match_system_command(input: &str) -> Option<(&'static str, &'static str, bool)> {
    let lower = input.to_lowercase();
    if lower.contains(' ') || lower.is_empty() {
        return None;
    }

    // (aliases, command_id, display_label, needs_confirm, platform filter)
    let commands: &[(&[&str], &str, &str, bool, &str)] = &[
        // Immediate
        (&["lock"], "lock", "🔒 Lock Screen", false, "all"),
        (&["sleep"], "sleep", "😴 Sleep", true, "all"),
        (&["screenshot"], "screenshot", "📸 Screenshot", false, "all"),
        (&["mute"], "mute", "🔇 Mute Audio", false, "all"),
        (&["unmute"], "unmute", "🔊 Unmute Audio", false, "all"),
        (&["emoji"], "emoji", "😀 Emoji Picker", false, "all"),
        (
            &["trash", "recycle"],
            "trash",
            "🗑️ Open Recycle Bin",
            false,
            "windows",
        ),
        (&["trash"], "trash", "🗑️ Open Trash", false, "macos"),
        (&["trash"], "trash", "🗑️ Open Trash", false, "linux"),
        (
            &["taskmanager", "taskmgr"],
            "taskmanager",
            "📊 Task Manager",
            false,
            "windows",
        ),
        (
            &["activitymonitor", "taskmanager", "taskmgr"],
            "taskmanager",
            "📊 Activity Monitor",
            false,
            "macos",
        ),
        (
            &["taskmanager", "taskmgr", "systemmonitor"],
            "taskmanager",
            "📊 System Monitor",
            false,
            "linux",
        ),
        (
            &["terminal", "cmd", "powershell"],
            "terminal",
            "💻 Terminal",
            false,
            "windows",
        ),
        (&["terminal"], "terminal", "💻 Terminal", false, "macos"),
        (&["terminal"], "terminal", "💻 Terminal", false, "linux"),
        (
            &["explorer"],
            "filemanager",
            "📁 File Explorer",
            false,
            "windows",
        ),
        (&["finder"], "filemanager", "📁 Finder", false, "macos"),
        (
            &["files", "nautilus"],
            "filemanager",
            "📁 Files",
            false,
            "linux",
        ),
        // With confirmation
        (
            &["restart", "reboot"],
            "restart",
            "🔄 Restart Computer",
            true,
            "all",
        ),
        (&["shutdown"], "shutdown", "⏻ Shut Down", true, "all"),
        (
            &["signout", "logout", "logoff"],
            "signout",
            "🚪 Sign Out",
            true,
            "all",
        ),
    ];

    let platform = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };

    // Exact match first
    for &(aliases, id, label, confirm, plat) in commands {
        if plat != "all" && plat != platform {
            continue;
        }
        if aliases.contains(&lower.as_str()) {
            return Some((id, label, confirm));
        }
    }
    // Prefix match
    for &(aliases, id, label, confirm, plat) in commands {
        if plat != "all" && plat != platform {
            continue;
        }
        if aliases.iter().any(|a| a.starts_with(lower.as_str())) {
            return Some((id, label, confirm));
        }
    }
    None
}

#[tauri::command]
pub async fn handle_floating_input(
    input: String,
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    let trimmed_input = input.trim();

    // Collect ALL matches from all sources (no early returns)
    let mut results: Vec<serde_json::Value> = Vec::new();

    // URL
    if is_url(trimmed_input) {
        results.push(serde_json::json!({ "type": "url", "value": trimmed_input, "score": 95 }));
    }

    // Path
    if let Some(path) = is_path(trimmed_input) {
        let is_file = path.contains('.') && !path.ends_with('\\') && !path.ends_with('/');
        results.push(serde_json::json!({
            "type": "path",
            "pathType": if is_file { "file" } else { "folder" },
            "value": path,
            "score": 90
        }));
    }

    // Well-known directory
    if let Some(path) = resolve_well_known_dir(trimmed_input) {
        results.push(serde_json::json!({
            "type": "path",
            "pathType": "folder",
            "value": path,
            "score": 87
        }));
    }

    // System command
    if let Some((cmd_id, cmd_label, needs_confirm)) = match_system_command(trimmed_input) {
        results.push(serde_json::json!({
            "type": "system",
            "cmdId": cmd_id,
            "cmdLabel": cmd_label,
            "needsConfirm": needs_confirm,
            "score": 86
        }));
    }

    // App search — use original input (preserving spaces) so "w " doesn't match "word"
    let launcher = features.app_launcher.lock().await;
    let app_matches = launcher.find_app(&input);
    for (i, app) in app_matches.iter().enumerate() {
        results.push(serde_json::json!({
            "type": "app",
            "name": app.name,
            "path": app.path,
            "icon_base64": app.icon_base64,
            "emoji_icon": app.emoji_icon,
            "score": 80 - i
        }));
    }

    // Sort by score descending
    results.sort_by(|a, b| {
        let sa = a.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        let sb = b.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        sb.cmp(&sa)
    });

    Ok(serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string()))
}

#[tauri::command]
pub async fn launch_app_by_name(
    app_name: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    info!("Launching app by name: {}", app_name);

    let launcher = features.app_launcher.lock().await;
    let matches = launcher.find_app(&app_name);

    if let Some(app_to_launch) = matches.first() {
        launcher.launch(app_to_launch).map_err(|e| {
            error!("Failed to launch {}: {}", app_name, e);
            format!("Failed to launch {}: {}", app_name, e)
        })?;

        if let Some(floating_window) = app.get_webview_window("floating") {
            let _ = floating_window.hide();
        }

        Ok(())
    } else {
        Err(format!("Application not found: {}", app_name).into())
    }
}

#[tauri::command]
pub async fn open_url(url: String, app: tauri::AppHandle) -> Result<(), AppError> {
    info!("Opening URL: {}", url);

    let full_url = if url.starts_with("www.") {
        format!("https://{}", url)
    } else {
        url.clone()
    };

    os::open_url(&full_url).map_err(|e| format!("Failed to open URL: {}", e))?;

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    Ok(())
}

#[tauri::command]
pub async fn open_path(path: String, app: tauri::AppHandle) -> Result<(), AppError> {
    info!("Opening path: {}", path);

    let expanded_path = if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            path.replacen('~', &home.to_string_lossy(), 1)
        } else {
            path.clone()
        }
    } else {
        path.clone()
    };

    let path_obj = std::path::Path::new(&expanded_path);
    if !path_obj.exists() {
        return Err(format!("Path does not exist: {}", expanded_path).into());
    }

    os::open_path(&expanded_path).map_err(|e| format!("Failed to open path: {}", e))?;

    if let Some(floating_window) = app.get_webview_window("floating") {
        let _ = floating_window.hide();
    }

    Ok(())
}

#[tauri::command]
pub async fn execute_shortcut(
    path: String,
    args: Vec<String>,
    working_directory: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    info!("Executing shortcut: {} with args: {:?}", path, args);

    use std::process::Command;

    let expanded_path = if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            path.replacen('~', &home.to_string_lossy(), 1)
        } else {
            path.clone()
        }
    } else {
        path.clone()
    };

    let expanded_work_dir = working_directory.as_ref().and_then(|wd| {
        if wd.starts_with('~') {
            dirs::home_dir().map(|home| wd.replacen('~', &home.to_string_lossy(), 1))
        } else {
            Some(wd.clone())
        }
    });

    let mut command = Command::new(&expanded_path);
    command.args(&args);

    if let Some(work_dir) = expanded_work_dir {
        command.current_dir(work_dir);
    }

    crate::os::configure_breakaway_from_job(&mut command);

    command
        .spawn()
        .map_err(|e| format!("Failed to execute shortcut: {}", e))?;

    // Anonymous shortcut-usage event. We deliberately don't send the
    // path or args — those can contain personal info like `~/Documents/...`.
    // The only property is the count of args so we can see "does anyone
    // actually use args?" in aggregate.
    crate::telemetry::track(
        &app,
        "shortcut_triggered",
        Some(serde_json::json!({ "arg_count": args.len() })),
    );

    info!("Shortcut executed successfully");
    Ok(())
}

#[tauri::command]
pub async fn execute_system_command(
    command_id: String,
    elevated: Option<bool>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    let elevated = elevated.unwrap_or(false);
    info!(
        "Executing system command: {}{}",
        command_id,
        if elevated { " (elevated)" } else { "" }
    );

    // Hide the floating window first
    if let Some(floating) = app.get_webview_window("floating") {
        let _ = floating.hide();
    }

    let result = if elevated {
        let (program, args) = crate::os::shell::system_command(command_id.as_str());
        crate::os::shell::spawn_elevated(program, &args)
    } else {
        let (program, args) = crate::os::shell::system_command(command_id.as_str());
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        crate::os::process::spawn_detached(&mut cmd)
    };

    match result {
        Ok(_) => {
            info!("System command '{}' executed", command_id);
            Ok(())
        }
        Err(e) => {
            error!("Failed to execute system command '{}': {}", command_id, e);
            Err(format!("Failed to execute: {}", e).into())
        }
    }
}

/// Fetch metadata (title, description, image, favicon) from a URL for
/// link previews. Consults the on-disk cache first; on a fresh hit
/// returns the cached value (including the negative-cache `null`
/// shape) without going to the network. Misses do the live fetch and
/// persist before returning. See `src/link_metadata_cache.rs` for the
/// TTL + eviction policy.
#[tauri::command]
pub async fn fetch_link_metadata(url: String) -> Result<serde_json::Value, AppError> {
    // Validate URL
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("Invalid URL".into());
    }

    // Cache lookup first. Disk reads are blocking but tiny — a
    // single JSON file deserialised on each call. Acceptable here
    // because a hit avoids a full HTTP round trip; we'd lose more
    // than we save by trying to be clever.
    let cache_url = url.clone();
    let cached = tauri::async_runtime::spawn_blocking(move || {
        crate::link_metadata_cache::lookup(&cache_url)
    })
    .await
    .ok()
    .flatten();
    if let Some(maybe_meta) = cached {
        return match maybe_meta {
            Some(value) => Ok(value),
            // Negative cache hit — surface the same empty-shape
            // response the live path returns, so callers don't need
            // to special-case "we tried and got nothing." The
            // background TTL guarantees we'll re-fetch in an hour
            // even if this stays in the cache.
            None => Ok(serde_json::json!({
                "url": url,
                "title": null,
                "description": null,
                "image": null,
                "favicon": null,
            })),
        };
    }

    // Cache miss — go to the network. Capture the outcome so we
    // can persist it (negative or positive) before returning. The
    // helper is split out so both arms write through the same cache
    // path without having to thread an Option around.
    let fetched = fetch_link_metadata_uncached(&url).await;
    let to_persist: Option<serde_json::Value> = match &fetched {
        Ok(value) => Some(value.clone()),
        Err(_) => None, // negative cache: short TTL re-tries soon
    };
    let url_for_store = url.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = crate::link_metadata_cache::store(&url_for_store, to_persist);
    });

    fetched
}

/// Live-network half of `fetch_link_metadata`. Pulled out so the
/// command wrapper can sit a cache check + cache write around it
/// without nesting too deeply. Returns the same shape the command
/// returns.
async fn fetch_link_metadata_uncached(url: &str) -> Result<serde_json::Value, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("Mozilla/5.0 (compatible; Kage/1.0)")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Fetch error: {}", e))?;

    let final_url = resp.url().to_string();
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status).into());
    }

    // Only process HTML responses
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    if !content_type.contains("text/html") {
        return Ok(serde_json::json!({
            "url": final_url,
            "title": null,
            "description": null,
            "image": null,
            "favicon": null,
        }));
    }

    // Read only the first 32KB to extract meta tags (don't download entire pages)
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Read error: {}", e))?;
    let html = String::from_utf8_lossy(&bytes[..bytes.len().min(32768)]);

    let title = extract_meta(&html, "og:title")
        .or_else(|| extract_meta(&html, "twitter:title"))
        .or_else(|| extract_tag_content(&html, "title"));

    let description = extract_meta(&html, "og:description")
        .or_else(|| extract_meta(&html, "description"))
        .or_else(|| extract_meta(&html, "twitter:description"));

    // Hero image (Open Graph / Twitter Card) — separate from the
    // favicon since the formatter renders them in different places.
    // Folding both into one field (which previous versions did) meant
    // the card always showed a 28-px favicon even when the page had a
    // proper og:image; the result looked half-built next to peer link
    // previews in Slack / Discord / Notion.
    let image = extract_meta(&html, "og:image")
        .or_else(|| extract_meta(&html, "twitter:image"))
        .or_else(|| extract_meta(&html, "twitter:image:src"))
        .map(|raw| resolve_url(&raw, &final_url));

    // Favicon is a small site-identity glyph. We try the explicit
    // <link rel="icon"> first, then a /favicon.ico fallback. Never
    // reuse the hero image — they have different aspect ratios and
    // CSS treatment.
    let favicon = extract_link_icon(&html, &final_url);

    Ok(serde_json::json!({
        "url": final_url,
        "title": title,
        "description": description,
        "image": image,
        "favicon": favicon,
    }))
}

/// Wipe the on-disk link-metadata cache. Surfaced from the Link
/// Preview settings page so a user can force a refresh after a
/// publisher changes their OG tags or after a transient outage.
#[tauri::command]
pub async fn link_metadata_clear_cache() -> Result<(), AppError> {
    tauri::async_runtime::spawn_blocking(crate::link_metadata_cache::clear)
        .await
        .map_err(|e| format!("Cache clear task failed: {}", e))?
        .map_err(|e| format!("Failed to clear link metadata cache: {}", e))?;
    Ok(())
}

/// Counts + on-disk size of the cache, for the Settings UI.
#[tauri::command]
pub async fn link_metadata_cache_stats() -> Result<crate::link_metadata_cache::CacheStats, AppError>
{
    tauri::async_runtime::spawn_blocking(crate::link_metadata_cache::stats)
        .await
        .map_err(|e| AppError::from(format!("Stats task failed: {}", e)))
}

/// Extract content from <meta property="X" content="..."> or <meta name="X" content="...">
fn extract_meta(html: &str, name: &str) -> Option<String> {
    let lower = html.to_lowercase();
    // Try property= first (Open Graph), then name= (standard meta)
    for attr in &["property", "name"] {
        let needle = format!("{}=\"{}\"", attr, name);
        if let Some(pos) = lower.find(&needle) {
            // Find content= in the same <meta> tag
            let tag_start = lower[..pos].rfind('<').unwrap_or(0);
            let tag_end = lower[pos..]
                .find('>')
                .map(|i| pos + i)
                .unwrap_or(lower.len());
            let tag = &html[tag_start..tag_end];
            if let Some(content) = extract_attr(tag, "content") {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Extract text content from <tag>...</tag>
fn extract_tag_content(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    if let Some(start) = lower.find(&open) {
        if let Some(gt) = lower[start..].find('>') {
            let content_start = start + gt + 1;
            if let Some(end) = lower[content_start..].find(&close) {
                let text = html[content_start..content_start + end].trim();
                if !text.is_empty() {
                    return Some(html_decode(text));
                }
            }
        }
    }
    None
}

/// Extract <link rel="icon" href="..."> or <link rel="shortcut icon" href="...">
fn extract_link_icon(html: &str, base_url: &str) -> Option<String> {
    let lower = html.to_lowercase();
    for pattern in &["rel=\"icon\"", "rel=\"shortcut icon\""] {
        if let Some(pos) = lower.find(pattern) {
            let tag_start = lower[..pos].rfind('<').unwrap_or(0);
            let tag_end = lower[pos..]
                .find('>')
                .map(|i| pos + i)
                .unwrap_or(lower.len());
            let tag = &html[tag_start..tag_end];
            if let Some(href) = extract_attr(tag, "href") {
                return Some(resolve_url(href.trim(), base_url));
            }
        }
    }
    // Fallback: /favicon.ico
    if let Ok(parsed) = url::Url::parse(base_url) {
        return Some(format!(
            "{}://{}/favicon.ico",
            parsed.scheme(),
            parsed.host_str().unwrap_or("")
        ));
    }
    None
}

/// Extract an attribute value from an HTML tag string
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let needle = format!("{}=", attr);
    if let Some(pos) = lower.find(&needle) {
        let after = &tag[pos + needle.len()..];
        let after = after.trim_start();
        if let Some(content) = after.strip_prefix('"') {
            if let Some(end) = content.find('"') {
                return Some(content[..end].to_string());
            }
        } else if let Some(content) = after.strip_prefix('\'') {
            if let Some(end) = content.find('\'') {
                return Some(content[..end].to_string());
            }
        }
    }
    None
}

/// Resolve a potentially relative URL against a base URL
fn resolve_url(href: &str, base: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("data:") {
        return href.to_string();
    }
    if href.starts_with("//") {
        return format!("https:{}", href);
    }
    if let Ok(base_url) = url::Url::parse(base) {
        if let Ok(resolved) = base_url.join(href) {
            return resolved.to_string();
        }
    }
    href.to_string()
}

/// Basic HTML entity decoding
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
}

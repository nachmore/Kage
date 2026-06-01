//! Auto-update system, backed by `tauri-plugin-updater`.
//!
//! The plugin handles the part that actually matters for security:
//! fetching a signed `latest.json` manifest, verifying the signature on
//! the installer with a compile-time public key, and running the right
//! per-OS install flow. This module wraps the plugin with the scheduling
//! and UX concerns the plugin doesn't care about:
//!
//!   - Channel-aware endpoint routing (`stable` / `beta` / `dev`).
//!   - Daily-check schedule and a "silent install on idle" gate so the
//!     user isn't interrupted mid-conversation.
//!   - Session resume across the install-and-restart boundary (a
//!     `last-session.txt` file the next launch picks up).
//!   - A `was_just_updated` flag the welcome banner consumes.
//!   - Changelog fetch for Settings → Updates.
//!
//! The old hand-rolled updater used to live here; its core flaw was no
//! signature check — a network-MITM attacker could swap the installer
//! for anything. This module keeps all of that old public API name
//! surface but delegates the actual network + install work to the
//! plugin, so every call site at main.rs / commands / setup stays
//! unchanged while the trust story gets correct-by-construction.

use crate::config::Config;
use crate::lock_ext::LockExt;
use anyhow::{Context, Result};
use log::{error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::Manager;
use tauri_plugin_updater::{Update, UpdaterExt};

/// Compile-time endpoint URLs per channel (from Cargo.toml
/// `[package.metadata.update]`). An empty value means the channel isn't
/// configured for this build — [`endpoint_for_channel`] falls back to
/// stable in that case.
pub const ENDPOINT_STABLE: &str = env!("UPDATE_ENDPOINT_STABLE");
pub const ENDPOINT_BETA: &str = env!("UPDATE_ENDPOINT_BETA");
pub const ENDPOINT_DEV: &str = env!("UPDATE_ENDPOINT_DEV");
/// Legacy raw-CHANGELOG.md URL. Still surfaced via `get_update_urls`
/// for diagnostic display, but no longer consumed by [`fetch_changelog`]
/// — that function now hits the GitHub Releases API so the in-app
/// changelog stays version-pinned and doesn't leak unreleased commits
/// from `main`.
pub const CHANGELOG_URL: &str = env!("UPDATE_CHANGELOG_URL");
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Optional compile-time updater public key. Provisioned by build.rs
/// from either `TAURI_UPDATER_PUBKEY` env or `.tauri-updater-pubkey`
/// file. Release builds fail the build if this is absent (we never ship
/// release binaries that can't verify updates); debug builds tolerate
/// `None` so the app still runs without update infra configured.
pub const PUBKEY: Option<&str> = option_env!("TAURI_UPDATER_PUBKEY");

/// Resolve a channel to its endpoint URL. An empty URL means the channel
/// isn't configured at compile time; we fall through to stable in that
/// case so the app still finds *some* manifest rather than failing the
/// daily check silently.
pub fn endpoint_for_channel(channel: crate::config::Channel) -> &'static str {
    let url = match channel {
        crate::config::Channel::Beta => ENDPOINT_BETA,
        crate::config::Channel::Dev => ENDPOINT_DEV,
        crate::config::Channel::Stable => ENDPOINT_STABLE,
    };
    if url.is_empty() {
        ENDPOINT_STABLE
    } else {
        url
    }
}

/// Shared state for the updater.
///
/// Stores the cached [`Update`] handle returned by the plugin's
/// `check()`. We keep it around (instead of re-checking right before
/// install) so the download + install sequence can be triggered the
/// moment the user is idle, without an extra network round trip that
/// might time out or change the available version.
pub struct UpdaterState {
    /// Timestamp of the last time any user-facing window (floating,
    /// chat, settings) was shown OR was observed visible by the
    /// updater idle loop. Continuously refreshed by the idle poll
    /// while any window is on screen, so the 5-minute "idle" gate
    /// can never trip while the user has UI open.
    pub last_user_activity: std::sync::Mutex<Instant>,
    /// True when `pending_update` holds an `Update` ready to install.
    pub update_ready: AtomicBool,
    /// The [`Update`] returned by the plugin when a newer version was
    /// found. `None` either because no check has happened yet, or the
    /// last check reported up-to-date.
    ///
    /// Wrapped in `Mutex<Option<...>>` (not `RwLock`) because the only
    /// access patterns are "take it out to install" or "swap in a new
    /// one after check" — read-heavy workloads don't exist here.
    pub pending_update: std::sync::Mutex<Option<Update>>,
    /// Cached version string from the last successful check.
    /// Surfaced to the Settings UI without re-checking.
    pub available_version: std::sync::Mutex<Option<String>>,
}

impl Default for UpdaterState {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdaterState {
    pub fn new() -> Self {
        Self {
            last_user_activity: std::sync::Mutex::new(Instant::now()),
            update_ready: AtomicBool::new(false),
            pending_update: std::sync::Mutex::new(None),
            available_version: std::sync::Mutex::new(None),
        }
    }

    /// Record that a user-facing window was just shown — or, from the
    /// idle loop, that one is currently still visible. The 5-minute
    /// idle gate measures "time since last touch", so refreshing this
    /// while any window is on screen keeps the gate closed for the
    /// duration of any visible UI session.
    pub fn touch_activity(&self) {
        if let Ok(mut t) = self.last_user_activity.lock() {
            *t = Instant::now();
        }
    }

    /// True when the user hasn't touched any user-facing window for
    /// 5+ minutes — the gate for silent auto-install so we don't
    /// yank the app out from under an active session.
    pub fn is_idle(&self) -> bool {
        self.last_user_activity
            .lock()
            .map(|t| t.elapsed().as_secs() >= 300)
            .unwrap_or(false)
    }
}

/// User-facing windows whose visibility blocks silent auto-install.
/// Excludes transient windows (context-menu, inline-assist) and
/// install-flow windows (welcome, store) — those either pop up and
/// disappear quickly or are explicitly out-of-band of regular use.
const USER_WINDOW_LABELS: &[&str] = &[
    crate::window_labels::FLOATING,
    crate::window_labels::MAIN,
    crate::window_labels::SETTINGS,
];

/// True if any of the user-facing windows is currently shown. Used by
/// the idle loop to refresh `last_user_activity` while UI is up
/// (preventing the 5-minute idle gate from tripping during a long
/// session) AND as a final guard before the install actually runs.
fn is_any_user_window_visible(app: &tauri::AppHandle) -> bool {
    use tauri::Manager;
    USER_WINDOW_LABELS.iter().any(|label| {
        app.get_webview_window(label)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false)
    })
}

/// Run a plugin `check()` for the given channel and return the resulting
/// `Update`. The plugin takes care of fetching the manifest, filtering
/// by target / arch / current version, and verifying the signature
/// coverage on the returned blob.
///
/// Pubkey resolution: if a compile-time key is present we pass it at
/// runtime via `updater_builder().pubkey(...)`. A missing key means we
/// don't check for updates at all — safer to silently no-op than to
/// ship updates with no verification.
///
/// Missing-manifest handling: a 404 from the channel endpoint isn't
/// an error — it means "no release has been cut on this channel yet."
/// Common in two scenarios:
///   - A new channel where CI hasn't run / hasn't published yet
///     (e.g. `dev-latest` before the first auto-publish).
///   - A user pointed at a stale alias that's been retired.
///
/// Either way the user-facing answer is "you're as up-to-date as you
/// can be on this channel" — same as the no-update case. Surfacing it
/// as an error confused users (the previous "[object Object]" bug
/// notwithstanding). The 404 stays in info-level logs for debugging.
pub async fn plugin_check(
    app: &tauri::AppHandle,
    channel: crate::config::Channel,
) -> Result<Option<Update>> {
    let Some(pubkey) = PUBKEY else {
        warn!("Updater: no public key configured — skipping check");
        return Ok(None);
    };

    let endpoint = endpoint_for_channel(channel);
    if endpoint.is_empty() {
        warn!(
            "Updater: no endpoint configured for channel '{}'",
            channel.as_str()
        );
        return Ok(None);
    }

    info!(
        "Checking for updates (channel={}, endpoint={})",
        channel.as_str(),
        endpoint
    );

    let endpoint_url = reqwest::Url::parse(endpoint)
        .with_context(|| format!("Invalid endpoint URL: {}", endpoint))?;

    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint_url])
        .context("Failed to configure updater endpoints")?
        .pubkey(pubkey)
        .build()
        .context("Failed to build updater")?;

    match updater.check().await {
        Ok(maybe) => Ok(maybe),
        Err(e) if is_manifest_not_found(&e) => {
            // Treat as "no update available" — see fn doc above.
            info!(
                "No release published on channel '{}' yet (404 at {}); reporting up-to-date",
                channel.as_str(),
                endpoint
            );
            Ok(None)
        }
        Err(e) => Err(anyhow::Error::new(e).context("Update check failed")),
    }
}

/// Heuristic: did this `tauri_plugin_updater::Error` come from a 404
/// response (manifest missing) vs. a real failure (network down,
/// signature verification, etc.)?
///
/// The plugin doesn't expose the HTTP status code as a typed variant;
/// the only information we get back is the error's `Display` string,
/// which contains "did not respond with a successful status code" for
/// any non-2xx, plus the URL and the status. We check for the canonical
/// "404" / "Not Found" tokens. If the plugin ever surfaces a typed
/// status accessor we should switch to that — for now the string match
/// is the only available signal.
fn is_manifest_not_found(e: &tauri_plugin_updater::Error) -> bool {
    let msg = e.to_string();
    msg.contains("404") || msg.to_lowercase().contains("not found")
}

/// Download + install a previously-checked `Update`. The plugin streams
/// bytes to a temp file, verifies the signature, and then runs the
/// platform installer. Verified against tauri-plugin-updater 2.10.1:
///
///   - Windows: spawns the NSIS installer and calls `process::exit(0)`
///     internally — this function never returns on Windows.
///   - macOS: extracts the new `.app.tar.gz`, swaps it on disk via
///     `fs::rename` (escalates to AppleScript admin if needed), then
///     RETURNS. The caller is responsible for exiting so the user
///     relaunches into the freshly-installed binary.
///   - Linux: not built for Kage (we don't ship Linux today).
///
/// Treat success as "process is about to exit" — even when this returns
/// on macOS, the right move is to call `app.exit(0)` immediately. The
/// running binary's executable was just replaced on disk; continuing
/// to run it produces undefined behaviour the moment any file inside
/// the bundle is referenced.
pub async fn plugin_download_and_install(app: &tauri::AppHandle, update: Update) -> Result<()> {
    info!(
        "Downloading update v{} (body: {:?})",
        update.version, update.body
    );
    let app_for_finish = app.clone();
    let result = update
        .download_and_install(
            |_, _| {},
            move || {
                info!("Update downloaded, starting installer");
                // Tear down our own children explicitly while the Job
                // Object is still in kill-on-close mode. Doing this
                // FIRST (before releasing the job) means a panic
                // anywhere in this block before exit still kills our
                // children — no orphan agent backend / TTS leaks. The
                // graceful_shutdown is the same path the tray Quit
                // uses; it hides windows, kills TTS, and flushes
                // the log.
                crate::commands::system::graceful_shutdown(&app_for_finish);
                // Disconnect the ACP client — kills the agent backend explicitly
                // rather than relying on stdin-EOF semantics or the
                // job kill (which is about to be released). Best
                // effort; if state isn't available we let the existing
                // implicit cleanup handle it.
                if let Some(acp) = app_for_finish.try_state::<crate::state::AcpHandles>() {
                    acp.client.disconnect();
                }
                // Now detach the installer-to-be from our Job Object.
                // The plugin's about to ShellExecuteW the installer;
                // without this it'd inherit the job and die the
                // moment our process exit closes the job's last
                // handle (kill-on-close fires for everything in the
                // job).
                //
                // Order matters: graceful_shutdown above ran while
                // kill-on-close was still active, so any panic
                // between then and `process::exit(0)` is still safe.
                // Once we cross THIS line, a panic could leak
                // children — but the closure has nothing else to do
                // and the plugin's next move is the spawn + exit
                // (no allocations, no I/O on shared state) so the
                // window is effectively zero.
                //
                // No-op on macOS/Linux — neither has the equivalent
                // kill-on-close mechanism we set up on Windows.
                crate::os::release_kill_on_exit_job();
                // Force the writer thread to flush before the plugin's
                // ShellExecuteW + process::exit(0). Without this, the
                // "starting installer" line can be lost on Windows
                // because the periodic 500ms flush hasn't fired yet
                // when the process tears down. graceful_shutdown
                // already flushes once, but more lines may have been
                // emitted since then so we flush again.
                crate::app_log::flush();
            },
        )
        .await;

    if let Err(e) = result {
        // Classify and translate to a user-readable message before
        // bubbling up. The previous generic
        // "Failed to download and install update" was true but
        // useless — the user couldn't tell whether their network had
        // dropped, the signature was bad, the disk was full, or the
        // installer needed admin rights. The variants below cover the
        // failures we've actually observed in telemetry +
        // bug reports; everything else falls through to the plugin's
        // own message (which usually carries the underlying IO /
        // reqwest detail).
        //
        // The signal is also recorded as a telemetry event so we can
        // see in aggregate which class of failure dominates.
        let reason = classify_install_error(&e);
        crate::telemetry::track(
            app,
            "update_install_failed",
            Some(serde_json::json!({ "reason": reason })),
        );
        return Err(format_install_error(&e, reason));
    }
    Ok(())
}

/// Coarse bucket for an install failure. Returned as a stable string
/// so telemetry aggregates cleanly across versions.
pub fn classify_install_error(e: &tauri_plugin_updater::Error) -> &'static str {
    let msg = e.to_string().to_lowercase();
    if msg.contains("signature")
        || msg.contains("verify")
        || msg.contains("public key")
        || msg.contains("minisign")
    {
        return "signature";
    }
    if msg.contains("403") || msg.contains("forbidden") {
        return "forbidden";
    }
    if msg.contains("404") || msg.contains("not found") {
        return "not_found";
    }
    if msg.contains("disk")
        || msg.contains("space")
        || msg.contains("os error 112") // Windows ERROR_DISK_FULL
        || msg.contains("os error 28")
    // Linux ENOSPC
    {
        return "disk_full";
    }
    if msg.contains("denied")
        || msg.contains("permission")
        || msg.contains("os error 5") // Windows ERROR_ACCESS_DENIED
        || msg.contains("os error 13")
    // Linux EACCES
    {
        return "permission";
    }
    if msg.contains("dns")
        || msg.contains("connect")
        || msg.contains("network")
        || msg.contains("timeout")
        || msg.contains("transport")
    {
        return "network";
    }
    if msg.contains("cancel") || msg.contains("interrupt") {
        return "cancelled";
    }
    "other"
}

/// Build the user-facing error message. Each known reason gets a
/// short explanation tailored to the most common cause; the
/// underlying error's `Display` is appended in parens so a power
/// user can still see the raw signal.
fn format_install_error(e: &tauri_plugin_updater::Error, reason: &'static str) -> anyhow::Error {
    let detail = e.to_string();
    let msg = match reason {
        "signature" => format!(
            "Update signature didn't verify. The download may be corrupted; try again. ({})",
            detail
        ),
        "forbidden" => format!(
            "Server refused the download (HTTP 403). If you're behind a proxy or filter, that's the most likely cause. ({})",
            detail
        ),
        "not_found" => format!(
            "Update file is missing on the server (HTTP 404). The release may have been pulled — try again later or check the channel in Settings → Updates. ({})",
            detail
        ),
        "disk_full" => format!(
            "Not enough disk space to download or install the update. ({})",
            detail
        ),
        "permission" => format!(
            "Kage doesn't have permission to write the installer file. Close any antivirus / EDR holding the directory and try again. ({})",
            detail
        ),
        "network" => format!(
            "Network error while downloading the update. Check your connection and try again. ({})",
            detail
        ),
        "cancelled" => format!("Update was cancelled. ({})", detail),
        _ => format!("Update install failed. ({})", detail),
    };
    anyhow::anyhow!(msg)
}

/// Maximum bytes of rendered markdown returned to the UI. Caps the
/// payload regardless of how chatty the release notes are — without
/// this, a 50-release fetch with verbose bodies could push hundreds of
/// KB of HTML into the settings webview and stutter the renderer.
const RELEASE_NOTES_BUDGET: usize = 30 * 1024;

/// How many releases to surface. Older history is still on GitHub; this
/// is the in-app "what changed recently" view, not a full archive.
const RELEASE_NOTES_LIMIT: usize = 10;

/// Parse `owner/repo` out of `CARGO_PKG_REPOSITORY`. Returns `None`
/// for any value that isn't a recognisable github.com URL — release
/// notes are GitHub-specific so a non-GitHub repo URL means we have
/// nothing to fetch.
fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let path = url
        .trim()
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.splitn(2, '/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// Format an ISO-8601 timestamp from the GitHub API into a short
/// human-readable date. Falls back to the raw input on parse failure
/// so we never strip useful information.
fn format_release_date(published_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(published_at)
        .map(|dt| dt.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|_| published_at.to_string())
}

/// Fetch the most recent releases from the GitHub API and render them
/// as a single markdown document, scoped to the user's channel.
///
/// Channel semantics:
///   - `stable` — only published releases where `prerelease=false`.
///   - `beta` / `dev` — include prereleases too. These channels track
///     rolling alias tags (`beta-latest`, `dev-latest`), so the user
///     is opted into seeing every cut, not just the curated stable
///     ones.
///
/// We fetch from `api.github.com/repos/{owner}/{repo}/releases` rather
/// than the raw `CHANGELOG.md` so the notes shown match the version
/// the user actually has, and an in-flight docs PR on `main` doesn't
/// leak unreleased prose into the about page.
///
/// Anonymous API calls are bound by GitHub's 60/hr per-IP rate limit,
/// which is plenty for the "open settings → fetch once" path. The
/// frontend already gates this on the changelog UI being visible.
pub fn fetch_changelog(channel: crate::config::Channel) -> Result<String> {
    let repo_url = env!("CARGO_PKG_REPOSITORY");
    let Some((owner, repo)) = parse_github_repo(repo_url) else {
        return Ok(format!(
            "No GitHub repository configured (got `{}`). Release notes are unavailable.",
            repo_url
        ));
    };

    let api_url = format!(
        "https://api.github.com/repos/{}/{}/releases?per_page=30",
        owner, repo
    );

    // GitHub rejects anonymous requests without a User-Agent and
    // expects the documented Accept header for the v3 API. Both
    // headers must be set or the response is a 403, not the JSON
    // payload we'd expect.
    let response = reqwest::blocking::Client::new()
        .get(&api_url)
        .header("User-Agent", format!("Kage/{}", CURRENT_VERSION))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .context("Failed to reach GitHub releases API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        // Surface the rate-limit case explicitly so the UI can show
        // something useful instead of a generic "fetch failed". 403
        // with `rate limit` in the body is the canonical signal.
        if status.as_u16() == 403 && body.to_lowercase().contains("rate limit") {
            return Ok(
                "GitHub API rate limit reached. Please try again in an hour, or view release notes on GitHub directly."
                    .to_string(),
            );
        }
        return Err(anyhow::anyhow!(
            "GitHub API returned {}: {}",
            status,
            body.chars().take(200).collect::<String>()
        ));
    }

    let releases: Vec<serde_json::Value> = response
        .json()
        .context("Failed to parse GitHub releases JSON")?;

    let include_prereleases = channel != crate::config::Channel::Stable;

    let mut rendered = String::new();
    let mut count = 0;

    for release in releases.iter() {
        if count >= RELEASE_NOTES_LIMIT {
            break;
        }

        // Skip drafts unconditionally — they're not visible to end
        // users and shouldn't show up in the in-app changelog.
        if release
            .get("draft")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let is_prerelease = release
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if is_prerelease && !include_prereleases {
            continue;
        }

        let name = release
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| release.get("tag_name").and_then(|v| v.as_str()))
            .unwrap_or("(untitled)");
        let published = release
            .get("published_at")
            .and_then(|v| v.as_str())
            .map(format_release_date);
        let body = release
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let html_url = release
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Heading line with a date suffix when available. The "[link]"
        // tail gives the user a direct route to the full release on
        // GitHub for assets, comments, etc.
        if let Some(date) = published {
            rendered.push_str(&format!("## {} — {}", name, date));
        } else {
            rendered.push_str(&format!("## {}", name));
        }
        if is_prerelease {
            rendered.push_str(" *(prerelease)*");
        }
        rendered.push('\n');

        if !html_url.is_empty() {
            rendered.push_str(&format!("[View on GitHub]({})\n\n", html_url));
        } else {
            rendered.push('\n');
        }

        if body.is_empty() {
            rendered.push_str("_No release notes._\n\n");
        } else {
            rendered.push_str(body);
            rendered.push_str("\n\n");
        }
        rendered.push_str("---\n\n");

        count += 1;

        // Cap total size to keep the webview snappy. We trim on a
        // UTF-8 boundary so the markdown parser doesn't trip over a
        // half-character at the end.
        if rendered.len() >= RELEASE_NOTES_BUDGET {
            let mut end = RELEASE_NOTES_BUDGET;
            while end > 0 && !rendered.is_char_boundary(end) {
                end -= 1;
            }
            rendered.truncate(end);
            rendered.push_str("\n\n*Older releases truncated. View the full history on GitHub.*\n");
            return Ok(rendered);
        }
    }

    if rendered.is_empty() {
        return Ok(format!(
            "No releases found for the **{}** channel yet.",
            channel.as_str()
        ));
    }

    Ok(rendered)
}

/// Persist the current session id so the post-restart process can
/// resume it. Written to `<config_dir>/kage/last-session.txt`, consumed
/// (and deleted) by `startup::resolve_resume_session_id`.
///
/// Semantics: this is "we're about to attempt an install" rather than
/// "we just installed successfully." We write it *before* calling
/// `download_and_install` because on Windows the plugin spawns the
/// installer and immediately `process::exit(0)`s — there's no return
/// path where we could persist the marker afterward. The cost is that
/// a failed install leaves a stale marker; the next launch will
/// auto-resume the user into their previous session, which is benign
/// (it's the session they were on anyway, not a foreign one). The
/// `last-session.txt` consumer deletes the file on every read so a
/// stale marker only fires once.
pub fn persist_resume_marker(session_id: Option<&str>) {
    if let Some(sid) = session_id {
        if let Ok(cfg_dir) = dirs::config_dir().context("config dir") {
            let marker = cfg_dir.join("kage").join("last-session.txt");
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&marker, sid) {
                Ok(()) => info!("Wrote resume marker to {:?}", marker),
                Err(e) => warn!("Failed to write resume marker: {}", e),
            }
        }
    }
}

/// Source of a triggered install — used to decide what UI to show on
/// the post-install relaunch. Persisted to disk via
/// `persist_install_source` because the installing process exits
/// before the new one starts; in-memory state doesn't survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallSource {
    /// User clicked "Install" in Settings or on the floating banner.
    /// Post-install: show the floating window with the "Kage updated!"
    /// banner so the user gets immediate feedback that their action
    /// completed.
    Interactive,
    /// Background idle-update applied without user interaction.
    /// Post-install: don't show the floating window. The banner stays
    /// queued and surfaces the next time the user manually summons
    /// the floating window.
    Idle,
}

impl InstallSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Idle => "idle",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "interactive" => Some(Self::Interactive),
            "idle" => Some(Self::Idle),
            _ => None,
        }
    }
}

/// Persist the install source so the post-install launch can decide
/// whether to show the floating window. Same write-before-install
/// pattern as `persist_resume_marker` — a failed install leaves the
/// marker behind, but `consume_install_source` deletes on read so a
/// stale marker only affects one launch and is harmless either way
/// (the worst case is we show or don't show the floating window once
/// when the user wasn't expecting it).
pub fn persist_install_source(source: InstallSource) {
    if let Ok(cfg_dir) = dirs::config_dir().context("config dir") {
        let marker = cfg_dir.join("kage").join("install-source.txt");
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&marker, source.as_str()) {
            Ok(()) => info!("Wrote install source marker: {}", source.as_str()),
            Err(e) => warn!("Failed to write install source marker: {}", e),
        }
    }
}

/// Read and delete the install-source marker. Returns `None` if no
/// marker is present (normal launch path) or if the marker is
/// unreadable / unparseable. Always deletes on read so a stale marker
/// can never leak into a future launch.
pub fn consume_install_source() -> Option<InstallSource> {
    let cfg_dir = dirs::config_dir()?;
    let marker = cfg_dir.join("kage").join("install-source.txt");
    if !marker.exists() {
        return None;
    }
    let contents = std::fs::read_to_string(&marker).ok();
    let _ = std::fs::remove_file(&marker);
    contents.as_deref().and_then(InstallSource::parse)
}

/// Start the background update checker loop.
///
/// Two tasks:
///  1. A periodic check that hits the plugin once per 24 hours (or the
///     first time if we've never checked). On success it caches the
///     `Update` handle; if the user has `silent_update` enabled it also
///     kicks off a background download + install when idle.
///  2. A minute-poll idle-watcher that pulls the cached `Update` out
///     and applies it once the user has been quiet for 5+ minutes.
pub fn start_update_loop(
    updater_state: Arc<UpdaterState>,
    config: Arc<std::sync::Mutex<Config>>,
    app_handle: tauri::AppHandle,
    window_sessions: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    acp_client: Arc<crate::acp_client::AcpClient>,
) {
    let updater_for_idle = updater_state.clone();
    let config_for_idle = config.clone();
    let app_for_idle = app_handle.clone();
    let window_sessions_for_idle = window_sessions;
    let _acp_client_for_idle = acp_client;

    tauri::async_runtime::spawn(async move {
        crate::os::set_current_thread_name("updater-check");
        // Initial delay — let the app finish starting before we hit the
        // network. Matters on slow networks where a failed check at
        // launch used to block tray-ready UI for 10+ seconds.
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;

        let mut first_check = true;

        loop {
            let (auto_check, should_check, silent_update, channel) = {
                let cfg = config.lock_or_recover();
                let auto = cfg.updates.auto_check;
                let should = if !auto {
                    false
                } else if first_check {
                    true
                } else {
                    cfg.updates.last_check_time.as_ref().is_none_or(|t| {
                        chrono::DateTime::parse_from_rfc3339(t)
                            .map(|dt| {
                                chrono::Utc::now().signed_duration_since(dt).num_hours() >= 24
                            })
                            .unwrap_or(true)
                    })
                };
                (auto, should, cfg.updates.silent_update, cfg.updates.channel)
            };

            if !auto_check || !should_check {
                first_check = false;
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                continue;
            }

            first_check = false;

            match plugin_check(&app_handle, channel).await {
                Ok(Some(update)) => {
                    let version = update.version.clone();
                    info!(
                        "Update available: {} (channel {})",
                        version,
                        channel.as_str()
                    );

                    if let Ok(mut v) = updater_state.available_version.lock() {
                        *v = Some(version.clone());
                    }
                    if let Ok(mut p) = updater_state.pending_update.lock() {
                        *p = Some(update);
                    }
                    updater_state.update_ready.store(true, Ordering::SeqCst);

                    // Notify the UI so the banner can light up.
                    crate::event_targets::emit_update_audience(
                        &app_handle,
                        crate::events::UPDATE_AVAILABLE,
                        &version,
                    );

                    if let Ok(mut cfg) = config.try_lock() {
                        cfg.updates.last_check_time = Some(chrono::Utc::now().to_rfc3339());
                        let _ = cfg.save();
                    }

                    let _ = silent_update; // silent_update is consumed by the idle loop below
                }
                Ok(None) => {
                    if let Ok(mut cfg) = config.try_lock() {
                        cfg.updates.last_check_time = Some(chrono::Utc::now().to_rfc3339());
                        let _ = cfg.save();
                    }
                }
                Err(e) => {
                    warn!("Update check failed: {}", e);
                    // Telemetry: surface check failures so we can spot
                    // a borked release endpoint or signature mismatch
                    // in aggregate. The reason bucket comes from a
                    // simple keyword scan of the error string — not
                    // perfect, but enough to distinguish "network was
                    // down" from "the signature didn't verify" which
                    // are very different things to investigate.
                    let msg = e.to_string().to_lowercase();
                    let reason = if msg.contains("signature") || msg.contains("verify") {
                        "signature"
                    } else if msg.contains("no endpoint") || msg.contains("not configured") {
                        "config"
                    } else if msg.contains("404") || msg.contains("not found") {
                        "not_found"
                    } else if msg.contains("dns")
                        || msg.contains("connect")
                        || msg.contains("network")
                        || msg.contains("timeout")
                    {
                        "network"
                    } else {
                        "other"
                    };
                    crate::telemetry::track(
                        &app_handle,
                        "update_check_failed",
                        Some(serde_json::json!({
                            "reason": reason,
                            "channel": channel.as_str(),
                        })),
                    );
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });

    // Idle-install loop: every minute, check if we have a pending
    // update AND the user is idle AND silent-update is enabled. If all
    // three, pull the Update out of the state and apply it. The
    // download+install runs on the Tokio runtime; the plugin exits the
    // process when the installer is handed off.
    tauri::async_runtime::spawn(async move {
        crate::os::set_current_thread_name("updater-idle");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;

            // If any user-facing window is currently on screen, refresh
            // the activity timestamp. This keeps the idle gate closed
            // for the duration of the session — even if the user hasn't
            // *triggered* a fresh show in the last 5 minutes (e.g.
            // they've had the chat window open the whole time).
            if is_any_user_window_visible(&app_for_idle) {
                updater_for_idle.touch_activity();
            }

            if !updater_for_idle.update_ready.load(Ordering::SeqCst) {
                continue;
            }
            if !updater_for_idle.is_idle() {
                continue;
            }
            // Belt + suspenders: even if the timer says idle, refuse to
            // install while a window is visible. Closes the race where
            // a user opens a window between the visibility refresh
            // above and the install decision below.
            if is_any_user_window_visible(&app_for_idle) {
                continue;
            }
            let silent = {
                let cfg = config_for_idle.lock_or_recover();
                cfg.updates.silent_update
            };
            if !silent {
                continue;
            }

            // Take ownership of the Update — install consumes it, and
            // even if it fails we don't want to retry forever on the
            // same stale handle (the plugin would happily re-verify it,
            // but a permanent error like "installer can't elevate"
            // shouldn't monopolize the idle window).
            let update = {
                let mut slot = updater_for_idle.pending_update.lock_or_recover();
                slot.take()
            };
            let Some(update) = update else {
                updater_for_idle.update_ready.store(false, Ordering::SeqCst);
                continue;
            };

            info!("User is idle, applying update...");

            // Stamp last_updated_version before the installer yanks the
            // process. Read via try_lock to avoid blocking behind a
            // long-running config save; if the lock is contended we
            // just skip the stamp — the next launch will still work,
            // we just won't show the "welcome back after update"
            // banner. Better than blocking the install.
            if let Ok(mut cfg) = config_for_idle.try_lock() {
                if let Ok(v) = updater_for_idle.available_version.lock() {
                    cfg.updates.last_updated_version = v.clone();
                }
                let _ = cfg.save();
            }

            // Write the resume marker so the restarted process picks
            // up the session the user was on. Prefer floating's session
            // (post-update banner shows the floating window first);
            // fall back to main's session.
            let session_id = window_sessions_for_idle.lock().ok().and_then(|m| {
                m.get(crate::window_labels::FLOATING)
                    .cloned()
                    .or_else(|| m.get(crate::window_labels::MAIN).cloned())
            });
            persist_resume_marker(session_id.as_deref());
            persist_install_source(InstallSource::Idle);

            match plugin_download_and_install(&app_for_idle, update).await {
                Ok(()) => {
                    // On Windows the plugin kills us before this
                    // returns. If we get here it's macOS: the plugin
                    // downloaded + installed into Applications and
                    // we're expected to quit or relaunch. Quit cleanly
                    // so launchd / the user restarts us with the new
                    // binary.
                    info!("Update installed; exiting to pick up new version");
                    app_for_idle.exit(0);
                }
                Err(e) => {
                    error!("Failed to install update: {}", e);
                    updater_for_idle.update_ready.store(false, Ordering::SeqCst);
                }
            }
        }
    });
}

/// Check if the app was just updated (current version matches
/// last_updated_version, meaning the process that stamped that field
/// is the one currently running).
pub fn was_just_updated(config: &Config) -> bool {
    config
        .updates
        .last_updated_version
        .as_ref()
        .map(|v| v == CURRENT_VERSION)
        .unwrap_or(false)
}

/// Clear the "just updated" flag after the user has been notified.
pub fn clear_update_flag(config: &mut Config) {
    config.updates.last_updated_version = None;
}

#[cfg(test)]
mod tests {
    // Pure-logic tests only — anything that touches the plugin's
    // Update / UpdaterBuilder types needs a real Tauri runtime, so
    // those paths are exercised by the integration tests instead.
    use super::*;

    #[test]
    fn parses_https_github_url() {
        assert_eq!(
            parse_github_repo("https://github.com/nachmore/Kage"),
            Some(("nachmore".into(), "Kage".into()))
        );
    }

    #[test]
    fn parses_https_github_url_with_trailing_slash() {
        assert_eq!(
            parse_github_repo("https://github.com/nachmore/Kage/"),
            Some(("nachmore".into(), "Kage".into()))
        );
    }

    #[test]
    fn parses_git_suffix() {
        assert_eq!(
            parse_github_repo("https://github.com/nachmore/Kage.git"),
            Some(("nachmore".into(), "Kage".into()))
        );
    }

    #[test]
    fn parses_ssh_url() {
        assert_eq!(
            parse_github_repo("git@github.com:nachmore/Kage.git"),
            Some(("nachmore".into(), "Kage".into()))
        );
    }

    #[test]
    fn rejects_non_github_url() {
        assert_eq!(parse_github_repo("https://gitlab.com/foo/bar"), None);
        assert_eq!(parse_github_repo(""), None);
        assert_eq!(parse_github_repo("https://github.com/"), None);
        assert_eq!(parse_github_repo("https://github.com/onlyowner"), None);
    }
}

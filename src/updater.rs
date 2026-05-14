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
use tauri::Emitter;
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

/// Valid channel values. Must stay in sync with the keys in
/// `[package.metadata.update]` and with the dropdown in
/// `ui/js/settings/updates.js`.
pub const VALID_CHANNELS: &[&str] = &["stable", "beta", "dev"];

/// Normalise a channel string to a known value. Unknown / empty input
/// collapses to `"stable"`. Used by `save_config` validation and by
/// `endpoint_for_channel` so both code paths agree on what counts as
/// valid.
pub fn normalize_channel(channel: &str) -> &'static str {
    let trimmed = channel.trim();
    for &known in VALID_CHANNELS {
        if known == trimmed {
            return known;
        }
    }
    "stable"
}

/// Resolve a channel string to its endpoint URL. Unknown values collapse
/// to stable via [`normalize_channel`] — a stale or corrupted config
/// shouldn't silently trap the user on a dead channel. An empty URL
/// means the channel isn't configured at compile time; we fall through
/// to stable in that case too.
pub fn endpoint_for_channel(channel: &str) -> &'static str {
    let url = match normalize_channel(channel) {
        "beta" => ENDPOINT_BETA,
        "dev" => ENDPOINT_DEV,
        _ => ENDPOINT_STABLE,
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
    /// Timestamp of the last time the floating window was shown.
    /// Updated from `commands::touch_floating_activity`.
    pub last_floating_activity: std::sync::Mutex<Instant>,
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
            last_floating_activity: std::sync::Mutex::new(Instant::now()),
            update_ready: AtomicBool::new(false),
            pending_update: std::sync::Mutex::new(None),
            available_version: std::sync::Mutex::new(None),
        }
    }

    /// Record that the floating window was just shown.
    pub fn touch_activity(&self) {
        if let Ok(mut t) = self.last_floating_activity.lock() {
            *t = Instant::now();
        }
    }

    /// True when the user hasn't touched the floating window for 5+
    /// minutes — the gate for silent auto-install so we don't yank the
    /// app out from under an active session.
    pub fn is_idle(&self) -> bool {
        self.last_floating_activity
            .lock()
            .map(|t| t.elapsed().as_secs() >= 300)
            .unwrap_or(false)
    }
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
pub async fn plugin_check(app: &tauri::AppHandle, channel: &str) -> Result<Option<Update>> {
    let Some(pubkey) = PUBKEY else {
        warn!("Updater: no public key configured — skipping check");
        return Ok(None);
    };

    let endpoint = endpoint_for_channel(channel);
    if endpoint.is_empty() {
        warn!("Updater: no endpoint configured for channel '{}'", channel);
        return Ok(None);
    }

    info!(
        "Checking for updates (channel={}, endpoint={})",
        channel, endpoint
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

    updater.check().await.context("Update check failed")
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
pub async fn plugin_download_and_install(update: Update) -> Result<()> {
    info!(
        "Downloading update v{} (body: {:?})",
        update.version, update.body
    );
    update
        .download_and_install(|_, _| {}, || info!("Update downloaded, starting installer"))
        .await
        .context("Failed to download and install update")?;
    Ok(())
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
pub fn fetch_changelog(channel: &str) -> Result<String> {
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

    let normalized = normalize_channel(channel);
    let include_prereleases = normalized != "stable";

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
            normalized
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
    floating_session_id: Arc<std::sync::Mutex<Option<String>>>,
    acp_client: Arc<crate::acp_client::AcpClient>,
) {
    let updater_for_idle = updater_state.clone();
    let config_for_idle = config.clone();
    let app_for_idle = app_handle.clone();
    let floating_session_for_idle = floating_session_id;
    let acp_client_for_idle = acp_client;

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
                (
                    auto,
                    should,
                    cfg.updates.silent_update,
                    cfg.updates.channel.clone(),
                )
            };

            if !auto_check || !should_check {
                first_check = false;
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                continue;
            }

            first_check = false;

            match plugin_check(&app_handle, &channel).await {
                Ok(Some(update)) => {
                    let version = update.version.clone();
                    info!("Update available: {} (channel {})", version, channel);

                    if let Ok(mut v) = updater_state.available_version.lock() {
                        *v = Some(version.clone());
                    }
                    if let Ok(mut p) = updater_state.pending_update.lock() {
                        *p = Some(update);
                    }
                    updater_state.update_ready.store(true, Ordering::SeqCst);

                    // Notify the UI so the banner can light up.
                    let _ = app_handle.emit("update_available", &version);

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
                            "channel": channel,
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

            if !updater_for_idle.update_ready.load(Ordering::SeqCst) {
                continue;
            }
            if !updater_for_idle.is_idle() {
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
            // up the session the user was on.
            let session_id = floating_session_for_idle
                .lock()
                .ok()
                .and_then(|s| s.clone())
                .or_else(|| acp_client_for_idle.get_session_id());
            persist_resume_marker(session_id.as_deref());

            match plugin_download_and_install(update).await {
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
    // NOTE: This module is currently unreachable because src/lib.rs
    // gates the updater module on `#[cfg(not(test))]`. Kept here so
    // that if the gating is ever relaxed (e.g. via a test-friendly
    // mock plugin) the parser invariants are already documented as
    // executable specs. Keep the test fns small and pure-logic so
    // they never need the runtime to compile.
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

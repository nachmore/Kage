//! Anonymous product analytics via Aptabase.
//!
//! # What gets sent
//!   - A random install ID (UUID v4, generated lazily the first time
//!     consent is confirmed, stored in `config.telemetry.install_id`).
//!   - App version, OS family + version, locale, engine (Tauri), coarse
//!     country derived from the IP at ingest. The IP itself is discarded
//!     by Aptabase and never stored.
//!   - The event name plus whatever string/number properties the call
//!     site explicitly passes. No prompts, no file paths, no clipboard
//!     contents, no PII.
//!
//! # What never gets sent
//!   - Anything the user typed, dictated, pasted, or loaded as an attachment.
//!   - File contents, file names, directory paths.
//!   - Session IDs, conversation history, agent responses.
//!   - Usernames, emails, device fingerprints, IP addresses.
//!
//! # Design notes
//!   - Every call site goes through [`track`], which reads the
//!     `telemetry.enabled` flag on the shared `Config` and returns
//!     immediately when disabled. The actual Aptabase plugin runs in its
//!     own background task inside tauri-plugin-aptabase, so even when
//!     enabled we never block the calling thread — the happy path is
//!     "grab lock, check bool, queue event, return".
//!   - If the build has no `APTABASE_KEY` (see [`APTABASE_KEY`]), the
//!     plugin is never registered and [`track`] short-circuits to a
//!     no-op. That means local dev builds don't ship telemetry, and
//!     forks don't accidentally send events to our dashboard.
//!   - The plugin requires opt-in via `aptabase:allow-track-event` in
//!     the capability manifest, so JS `trackEvent(...)` calls also
//!     route through the permission system.
//!   - We explicitly DO NOT track anything before the user has
//!     completed the welcome flow. `set_consent` is the single place
//!     that flips `telemetry.enabled` to `true` and sets the consent
//!     version after the welcome step.

use crate::config::Config;
use crate::lock_ext::LockExt;
use crate::state::FeatureServices;
use log::{debug, info};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_aptabase::EventTracker;

/// Current privacy policy version. Bump when the disclosed data
/// collection scope or recipients change. The UI compares this to
/// `config.telemetry.consent_version` and re-prompts users whose stored
/// consent is stale.
pub const PRIVACY_POLICY_VERSION: u32 = 1;

/// Compile-time analytics key. Provided by `build.rs` from either:
///   1. The `APTABASE_KEY` environment variable (used by CI), or
///   2. The gitignored `.aptabase-key` file at the repo root.
///
/// Internally we refer to it through this one const so a future provider
/// swap only needs to re-point it at whatever env var / file the next
/// vendor uses.
///
/// If absent (local dev, third-party forks), the plugin is never
/// registered and every [`track`] call is a cheap no-op. The key itself
/// is not secret — it's a public identifier — but we still gate on its
/// presence so dev builds don't pollute the production dataset and
/// third-party forks don't accidentally send events to our dashboard.
pub const APTABASE_KEY: Option<&str> = option_env!("APTABASE_KEY");

/// Returns true if this build can send telemetry at all (has a
/// compile-time key) AND the user has opted in.
///
/// Takes the shared `Arc<Mutex<Config>>` by reference and grabs a brief
/// lock — call sites don't have to clone.
///
/// **Consent contract** — the `install_id.is_some()` check is
/// load-bearing, not decorative. `enabled` defaults to `true` so that
/// users completing the welcome flow see their decision applied
/// immediately, but the ID is only generated once [`set_consent`] runs
/// from the welcome step or the Settings toggle. That means a brand-new
/// user who hasn't yet reached the consent step has `enabled=true` /
/// `install_id=None` → no events. Do not relax this check (e.g. by
/// lazily generating an ID inside [`track`]) without first reworking the
/// welcome-screen UX; doing so would silently opt users in before they
/// see the disclosure.
fn is_allowed(config: &Arc<std::sync::Mutex<Config>>) -> bool {
    if APTABASE_KEY.is_none() {
        return false;
    }
    let cfg = config.lock_or_recover();
    cfg.telemetry.enabled && cfg.telemetry.install_id.is_some()
}

/// Fire an anonymous event. Cheap no-op if telemetry is disabled or the
/// build has no key.
///
/// `props` must contain only string or number values — Aptabase rejects
/// arrays and nested objects. Pass `None` for events that don't carry
/// any properties (which should be most of them).
///
/// # Examples
///
/// ```ignore
/// telemetry::track(&app, "shortcut_triggered", None);
/// telemetry::track(&app, "extension_installed", Some(json!({
///     "extension_id": manifest.id,
/// })));
/// ```
pub fn track(app: &AppHandle, event: &str, props: Option<Value>) {
    let Some(features) = app.try_state::<FeatureServices>() else {
        return;
    };
    if !is_allowed(&features.config) {
        return;
    }
    // The Aptabase plugin enqueues asynchronously; this call returns
    // immediately. We intentionally ignore the Result — telemetry
    // errors are never worth surfacing to the user.
    let _ = app.track_event(event, props);
    debug!("Telemetry event: {}", event);
}

/// Record that the app just started. Fires exactly one of:
///   - `app_installed` — first launch after install (no prior `last_seen_version`)
///   - `app_upgraded`  — version number changed since last launch
///   - `app_started`   — steady-state launch
///
/// Also fires `app_daily_active` once per UTC day per install, so DAU /
/// MAU reports stay meaningful even for users who bounce the app many
/// times a day.
///
/// Runs inside the existing setup phase — not on a background thread —
/// so it can update the config before any other telemetry fires.
pub fn record_startup_events(app: &AppHandle, config: &Arc<std::sync::Mutex<Config>>) {
    if APTABASE_KEY.is_none() {
        return;
    }
    // Read + decide under a short-lived lock, then release before tracking
    // so the call doesn't contend with any concurrent config save.
    let (event_to_fire, should_daily, new_version, new_date) = {
        let mut cfg = config.lock_or_recover();
        if !cfg.telemetry.enabled || cfg.telemetry.install_id.is_none() {
            return;
        }
        let current = env!("CARGO_PKG_VERSION").to_string();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let event = match cfg.telemetry.last_seen_version.as_deref() {
            None => "app_installed",
            Some(prev) if prev != current => "app_upgraded",
            _ => "app_started",
        };
        let prev_version = cfg.telemetry.last_seen_version.clone();
        let daily = cfg.telemetry.last_daily_ping.as_deref() != Some(&today);

        cfg.telemetry.last_seen_version = Some(current.clone());
        if daily {
            cfg.telemetry.last_daily_ping = Some(today.clone());
        }
        let _ = cfg.save();
        (event, daily, current, prev_version)
    };

    // app_started / app_installed / app_upgraded — include version
    // transition when applicable.
    let props = match (event_to_fire, new_date) {
        ("app_upgraded", Some(prev)) => Some(json!({
            "from_version": prev,
            "to_version": new_version,
        })),
        _ => None,
    };
    let _ = app.track_event(event_to_fire, props);
    info!("Telemetry: {}", event_to_fire);

    if should_daily {
        let _ = app.track_event("app_daily_active", None);
    }
}

/// Record that the app is exiting. Intended for the `RunEvent::Exit`
/// hook in main.rs — must run to completion before the process dies, so
/// we explicitly flush after tracking.
pub fn record_shutdown(handler: &AppHandle) {
    if APTABASE_KEY.is_none() {
        return;
    }
    let Some(features) = handler.try_state::<FeatureServices>() else {
        return;
    };
    if !is_allowed(&features.config) {
        return;
    }
    let _ = handler.track_event("app_exited", None);
    // Block briefly to let the final HTTP POST complete before shutdown.
    handler.flush_events_blocking();
}

/// Apply a consent decision from the welcome flow or the Settings →
/// Privacy page. Called by the `set_telemetry_enabled` Tauri command
/// and `complete_first_run`.
///
/// When enabling, generates an install_id if one doesn't exist yet.
/// When disabling, leaves the install_id in place — that way the user
/// can re-enable without being counted as a new install, and deletion
/// requests can still target a known ID.
pub fn set_consent(config: &Arc<std::sync::Mutex<Config>>, enabled: bool) {
    let mut cfg = config.lock_or_recover();
    cfg.telemetry.enabled = enabled;
    if enabled && cfg.telemetry.install_id.is_none() {
        cfg.telemetry.install_id = Some(uuid::Uuid::new_v4().to_string());
    }
    cfg.telemetry.consent_version = PRIVACY_POLICY_VERSION;
    let _ = cfg.save();
}

/// Generate a fresh install ID, orphaning all prior events from this
/// install. Exposed as the `reset_telemetry_install_id` command.
pub fn reset_install_id(config: &Arc<std::sync::Mutex<Config>>) -> String {
    let new_id = uuid::Uuid::new_v4().to_string();
    let mut cfg = config.lock_or_recover();
    cfg.telemetry.install_id = Some(new_id.clone());
    // Reset the last_seen_version so the next launch reports as a fresh
    // install under the new ID. This prevents a user resetting their ID
    // and then vanishing entirely from our reports.
    cfg.telemetry.last_seen_version = None;
    cfg.telemetry.last_daily_ping = None;
    let _ = cfg.save();
    new_id
}

/// Snapshot of the current telemetry settings, for the Settings UI.
#[derive(Debug, serde::Serialize)]
pub struct TelemetryInfo {
    pub enabled: bool,
    pub install_id: Option<String>,
    pub consent_version: u32,
    pub current_policy_version: u32,
    /// True if the build has a compile-time Aptabase key. When false,
    /// no telemetry can be sent regardless of the enabled flag, and the
    /// Settings UI should surface that.
    pub transport_available: bool,
}

pub fn snapshot(config: &Arc<std::sync::Mutex<Config>>) -> TelemetryInfo {
    let cfg = config.lock_or_recover();
    TelemetryInfo {
        enabled: cfg.telemetry.enabled,
        install_id: cfg.telemetry.install_id.clone(),
        consent_version: cfg.telemetry.consent_version,
        current_policy_version: PRIVACY_POLICY_VERSION,
        transport_available: APTABASE_KEY.is_some(),
    }
}

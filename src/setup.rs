//! Tauri setup helpers extracted from main()'s `.setup(...)` closure.
//!
//! Each function here runs once during Tauri application setup and
//! owns one concern (window configuration, hotkey hot-reload, etc.).
//! Moving them out of main.rs keeps the closure readable and
//! gives each stage a place to grow.
//!
//! These can't be unit-tested without spinning up a Tauri app, so the
//! trade is: small, self-explanatory functions with doc comments for
//! each concern, verified by building the binary and exercising it
//! manually.

use crate::events;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, ChildProcesses, FeatureServices, UiState};
use crate::window_labels::{self, is_session_host_label};
use log::{error, info, warn};
use std::sync::Arc;
use tauri::{App, AppHandle, Listener, Manager};

/// Configure the three transparent Tauri windows created by the app
/// config (floating, context-menu, inline-assist). Missing windows
/// are logged but not fatal — if e.g. the floating window failed to
/// register we want to know about it, not crash setup.
pub fn configure_transparent_windows(app: &App) {
    if let Some(floating_window) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating_window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = floating_window.set_shadow(false);
    } else {
        error!("Floating window not found during setup — UI will be limited");
    }

    if let Some(ctx_menu) = app.get_webview_window(window_labels::CONTEXT_MENU) {
        let _ = ctx_menu.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = ctx_menu.set_shadow(false);
    }

    if let Some(ia_win) = app.get_webview_window(window_labels::INLINE_ASSIST) {
        let _ = ia_win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = ia_win.set_shadow(false);
    }
}

/// Listen on `config_updated` and re-register all global hotkeys when
/// any of the three hotkey fields (main, clipboard, inline-assist)
/// actually changes. Snapshots the prior values so unrelated config
/// saves don't churn the registration.
pub fn install_hotkey_hot_reload(app: &App, initial_config: &crate::config::Config) {
    /// Snapshot of the three hotkey strings (main, clipboard, inline-assist).
    /// Aliased so the type doesn't bloat the local declaration.
    type HotkeySnapshot = (String, Option<String>, Option<String>);

    let hotkey_app = app.handle().clone();
    let hotkey_config = app.state::<FeatureServices>().config.clone();
    let last_hotkey_snapshot: Arc<std::sync::Mutex<HotkeySnapshot>> = {
        let main = initial_config.get_hotkey_string();
        let cb = initial_config.get_clipboard_hotkey_string();
        let ia = initial_config.get_inline_assist_hotkey_string();
        Arc::new(std::sync::Mutex::new((main, cb, ia)))
    };
    app.listen(events::CONFIG_UPDATED, move |_| {
        // Read the new hotkey strings out under a brief lock, then drop the
        // guard before doing anything else. Using lock() (via lock_or_recover)
        // instead of try_lock means we wait briefly under contention rather
        // than silently dropping the change. Pre-fix this listener used
        // try_lock and a single concurrent save of any config field would
        // make the user's hotkey edit go nowhere with no log line.
        let (new_main, new_cb, new_ia) = {
            let config = hotkey_config.lock_or_recover();
            (
                config.get_hotkey_string(),
                config.get_clipboard_hotkey_string(),
                config.get_inline_assist_hotkey_string(),
            )
        };

        let snapshot = last_hotkey_snapshot.lock_or_recover();
        if snapshot.0 == new_main && snapshot.1 == new_cb && snapshot.2 == new_ia {
            return;
        }

        info!("Hotkeys changed — re-registering all");
        // Drop the snapshot guard before calling register_all_hotkeys — that
        // path takes its own config lock and we don't want to hold an
        // unrelated mutex across it.
        let to_store = (new_main, new_cb, new_ia);
        drop(snapshot);
        crate::commands::system::register_all_hotkeys(&hotkey_app);
        *last_hotkey_snapshot.lock_or_recover() = to_store;
    });
}

/// Route `show-sessions` events (fired by the single-instance IPC
/// listener when a second launch tries to open) into the most-recently
/// focused chat window. Falls back to `main` if no chat window has been
/// focused this session — a fresh install or one where the user only
/// ever uses the floating widget.
pub fn install_show_sessions_listener(app: &App) {
    let app_handle = app.handle().clone();
    app.listen(events::SHOW_SESSIONS, move |_| {
        let handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let target_label = handle
                .try_state::<UiState>()
                .and_then(|ui| ui.last_focused_chat.lock().ok().and_then(|s| s.clone()))
                .filter(|label| handle.get_webview_window(label).is_some())
                .unwrap_or_else(|| window_labels::MAIN.to_string());
            info!(
                "show_sessions event received, surfacing window: {}",
                target_label
            );
            if target_label == window_labels::MAIN {
                if let Err(e) = crate::commands::window::open_chat_window(handle.clone()).await {
                    log::error!("Failed to open chat window from IPC signal: {}", e);
                }
            } else if let Some(window) = handle.get_webview_window(&target_label) {
                let _ = window.show();
                let _ = window.set_focus();
                crate::setup::update_activation_policy(&handle);
            }
        });
    });
}

/// Self-register the `kage://` URL scheme with the OS and route
/// incoming deep links to the store window.
///
/// Two arrival paths funnel into the same handler:
///   1. Cold launch — Kage wasn't running, the OS spawned us with the
///      URL on argv. The deep-link plugin parses it; we read the
///      result via `get_current()` here at setup time.
///   2. Warm launch — Kage was already up, the OS forwarded the URL
///      via single-instance (which has the `deep-link` feature flag,
///      so the URL lands in the deep-link plugin's channel rather
///      than just in argv). The plugin fires `deep-link://new-url`
///      via `on_open_url`.
///
/// Both paths call `handle_deep_link_url`, which:
///   - parses `kage://install/<id>`,
///   - opens the store window,
///   - emits `DEEP_LINK_INSTALL` with the id so the store JS can
///     scroll to + auto-prompt the install.
///
/// `register_all()` is called at the top because it's cheap and
/// idempotent. On Windows it writes HKCU keys pointing at this exe;
/// re-running with a moved binary path keeps the registration in
/// sync. On macOS the bundler handles registration via Info.plist
/// so this is a no-op (the plugin's macOS impl is empty).
pub fn install_deep_link_handler(app: &App) {
    use tauri_plugin_deep_link::DeepLinkExt;

    // Idempotent self-registration. If this fails (e.g. registry write
    // denied on a locked-down corporate machine) we log and carry on —
    // the user can still side-load extensions via the store window's
    // .zip flow, just without the one-click web path.
    let dl = app.deep_link();
    if let Err(e) = dl.register_all() {
        warn!("deep-link: failed to register `kage://` scheme: {e}");
    }

    // Cold-launch URLs. The plugin populated `current` from argv if
    // the process started with one; an empty list is fine.
    if let Ok(Some(urls)) = dl.get_current() {
        for url in urls {
            handle_deep_link_url(app.handle(), &url);
        }
    }

    // Warm-launch URLs. Fires for every subsequent kage:// click.
    let app_handle = app.handle().clone();
    dl.on_open_url(move |event| {
        for url in event.urls() {
            handle_deep_link_url(&app_handle, &url);
        }
    });
}

/// Parse a `kage://...` URL and dispatch it. Today the only verb is
/// `install`, so unknown shapes log + drop. Adding a new verb is a
/// match arm here plus a frontend listener.
fn handle_deep_link_url(app: &AppHandle, url: &url::Url) {
    if url.scheme() != "kage" {
        warn!("deep-link: ignoring unexpected scheme '{}'", url.scheme());
        return;
    }

    // We accept both `kage://install/<id>` (host=install, path=/<id>)
    // and `kage:install/<id>` (host empty, path=install/<id>) —
    // browsers and OS handlers vary on how they parse non-standard
    // schemes.
    let host = url.host_str().unwrap_or("");
    let path = url.path().trim_start_matches('/');
    let (verb, rest) = if !host.is_empty() {
        (host.to_string(), path.to_string())
    } else {
        // `kage:install/foo` → path is "install/foo"
        let mut parts = path.splitn(2, '/');
        let head = parts.next().unwrap_or("").to_string();
        let tail = parts.next().unwrap_or("").to_string();
        (head, tail)
    };

    match verb.as_str() {
        "install" => {
            // The id MUST match the validator's pattern
            // (`^[a-z0-9][a-z0-9_-]{0,63}$`) to land as a real
            // extension — anything outside that wouldn't survive the
            // catalog build. Be defensive anyway and reject obvious
            // junk so a malicious `kage://install/../../../etc/passwd`
            // can't push surprising payloads at the frontend.
            // No need to percent-decode: every char allowed in a valid
            // extension id (lowercase alnum + `_-`) is already
            // unreserved per RFC 3986, so it can't be encoded by a
            // well-behaved client. If anything came through
            // percent-encoded (e.g. an attacker wrote `%2e%2e` to mean
            // `..`), the safety check below rejects it because `%`
            // isn't in the allowed character set.
            let id = rest;
            if id.is_empty() || !is_safe_extension_id(&id) {
                warn!("deep-link: install URL has invalid id '{}', ignoring", id);
                return;
            }
            info!("deep-link: open store with install intent id='{}'", id);
            // The intent rides on the URL query param
            // (`store.html?tab=extensions&install=<id>`) for fresh
            // window creates; for already-open windows the command
            // does an eval_script call into the store's
            // handleDeepLinkInstall function. Either way the
            // store-side bootstrap reads the id and triggers the
            // install — no emit/listen race.
            let app_for_open = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = crate::commands::extensions::open_store_window_with_intent(
                    app_for_open,
                    Some("extensions".to_string()),
                    Some(id),
                )
                .await
                {
                    warn!("deep-link: failed to open store window: {e}");
                }
            });
        }
        other => {
            warn!("deep-link: unknown verb '{}' in URL '{}'", other, url);
        }
    }
}

/// Mirrors the validator pattern from Kage-Extensions
/// (`^[a-z0-9][a-z0-9_-]{0,63}$`). We re-implement here rather than
/// pulling regex in just for this one check because the rule is small
/// and stable, and avoiding regex keeps cold-start cheap. Keep in sync
/// if the catalog ever loosens its id rule.
fn is_safe_extension_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 64 {
        return false;
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Install a focus listener on `main` so the
/// `UiState.last_focused_chat` tracker stays accurate without each
/// chat-* peer having to coordinate with main. `chat-<uuid>` peers
/// install their own listener in `open_new_chat_window`.
pub fn install_main_focus_tracker(app: &App) {
    if let Some(window) = app.get_webview_window(window_labels::MAIN) {
        let app_handle = app.handle().clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Focused(true)) {
                crate::commands::window::mark_focused_chat(&app_handle, window_labels::MAIN);
            }
        });
    }
}

/// Boot the automation scheduler in the background and stash its
/// signal sender in FeatureServices so emit_automation_signal can find it.
pub fn spawn_automation_scheduler(app: &App) {
    let features: tauri::State<'_, FeatureServices> = app.state();
    let config_arc = features.config.clone();
    let signal_tx_arc = features.automation_signal_tx.clone();
    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let (scheduler, signal_rx) = crate::automation::AutomationScheduler::new(config_arc);
        *signal_tx_arc.lock_or_recover() = Some(scheduler.signal_sender());
        scheduler.run(signal_rx, app_handle).await;
    });
}

/// If Pocket TTS is configured to auto-start, spawn its Python server
/// in the background and stash the child handle in ChildProcesses so we
/// can shut it down later.
pub fn maybe_autostart_pocket_tts(app: &App, config: &crate::config::Config) {
    if !(config.pocket_tts.enabled && config.pocket_tts.auto_start && config.pocket_tts.installed) {
        return;
    }
    info!("Pocket TTS auto-start enabled, spawning server in background");
    let features: tauri::State<'_, FeatureServices> = app.state();
    let procs: tauri::State<'_, ChildProcesses> = app.state();
    let config_arc = features.config.clone();
    let tts_proc = procs.pocket_tts.clone();
    tauri::async_runtime::spawn(async move {
        let (port, voice, temp, eos_threshold, python) = {
            let config = config_arc.lock_or_recover();
            (
                config.pocket_tts.port,
                config.pocket_tts.voice.clone(),
                config.pocket_tts.temp,
                config.pocket_tts.eos_threshold,
                config
                    .pocket_tts
                    .python_path
                    .clone()
                    .unwrap_or_else(|| "python".to_string()),
            )
        };

        let script_path = crate::commands::pocket_tts::get_server_script_path();
        if !script_path.exists() {
            warn!("Pocket TTS server script not found, skipping auto-start");
            return;
        }

        let mut cmd = std::process::Command::new(&python);
        cmd.arg(script_path.to_str().unwrap_or(""))
            .args(["--port", &port.to_string()])
            .args(["--voice", &voice])
            .args(["--temp", &temp.to_string()])
            .args(["--eos-threshold", &eos_threshold.to_string()])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        crate::commands::pocket_tts::configure_no_window(&mut cmd);

        match cmd.spawn() {
            Ok(child) => {
                info!("Pocket TTS server auto-started (PID: {})", child.id());
                let mut proc = tts_proc.lock_or_recover();
                *proc = Some(child);
            }
            Err(e) => warn!("Failed to auto-start Pocket TTS server: {}", e),
        }
    });
}

/// Kick off the background app-registry scan: one scan now, then a
/// periodic refresh every hour so discovered apps stay fresh. Both
/// scans run on blocking threads so the async runtime isn't tied up
/// during Windows registry walks.
pub fn spawn_app_registry_scan(app: &App) {
    let features: tauri::State<'_, FeatureServices> = app.state();
    let launcher = features.app_launcher.clone();
    tauri::async_runtime::spawn(async move {
        crate::os::set_current_thread_name("app-launcher");

        match tauri::async_runtime::spawn_blocking(crate::app_launcher::AppLauncher::build_registry)
            .await
        {
            Ok(Ok(registry)) => {
                launcher.lock().await.apply_registry(registry);
            }
            Ok(Err(e)) => log::error!("Background app scan failed: {}", e),
            Err(e) => log::error!("Background app scan task failed: {}", e),
        }

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.tick().await; // skip immediate first tick
        loop {
            interval.tick().await;
            log::info!("Periodic app registry refresh");
            match tauri::async_runtime::spawn_blocking(
                crate::app_launcher::AppLauncher::build_registry,
            )
            .await
            {
                Ok(Ok(registry)) => {
                    launcher.lock().await.apply_registry(registry);
                }
                Ok(Err(e)) => log::error!("Periodic app scan failed: {}", e),
                Err(e) => log::error!("Periodic app scan task failed: {}", e),
            }
        }
    });
}

/// Self-heal the computer-control MCP registration on every launch.
///
/// Why this exists: `mcp_registration::ensure_registered()` only ran on
/// first-run opt-in or explicit toggle. After an update, three things
/// can drift the entry out of sync silently:
///   1. The exe install path changed (per-user reinstall, manual move).
///   2. The agent backend changed (e.g. user switched from Kiro to
///      Claude Code) so we'd want to write to a different mcp.json.
///   3. The bundle stopped shipping the MCP binary (build regression).
///
/// In all three cases the toggle in Settings says "on" but the agent
/// doesn't actually spawn the server, and there's no surface telling
/// the user why. This re-runs the registration on every launch so the
/// path stays fresh, and emits a loud log line if the binary isn't
/// where we expect it.
///
/// Cheap: a no-op when the existing entry already matches the current
/// path, which is the common case.
pub fn refresh_mcp_registration_if_enabled() {
    if !crate::mcp_registration::is_registered() {
        // User never opted in (or explicitly toggled off). Nothing to
        // refresh; leave their mcp.json alone.
        return;
    }

    match crate::mcp_registration::get_mcp_binary_path() {
        Some(path) => {
            info!(
                "computer-control MCP binary at {} — refreshing registration",
                path.display()
            );
            crate::mcp_registration::ensure_registered();
        }
        None => {
            // Toggle says on but we can't find the binary next to the
            // exe. Most likely a botched install/update; surface it
            // loudly so the user can see why the agent isn't getting
            // computer-control tools.
            warn!(
                "computer-control MCP is enabled in mcp.json but \
                 kage-computer-control-mcp binary is missing next to the \
                 main exe. The agent will fail to spawn it. Try toggling \
                 the switch in Settings → MCP Servers, or reinstall."
            );
        }
    }
}

/// Refresh the on-disk changelog cache in the background so the MCP
/// sidecar's `get_kage_changelog` tool can answer "what changed in the
/// last update?" from local disk — offline, and without giving the
/// sidecar an HTTP client.
///
/// Refresh policy: fetch when the cached version differs from the
/// running version (i.e. first launch after an upgrade — exactly when
/// the user is most likely to ask), when the cached channel differs
/// (user switched channels), or when the cache is missing/unreadable.
/// Steady-state launches with a fresh cache are a no-op. Fetch failure
/// is logged and the stale cache (if any) is kept — worst case the
/// agent reports slightly-old notes.
pub fn spawn_changelog_cache_refresh(app: &App) {
    let config: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::config::Config>>> =
        app.state();
    let channel = config.lock_or_recover().updates.channel;
    tauri::async_runtime::spawn(async move {
        let current_version = env!("CARGO_PKG_VERSION");
        if let Some(cache) = kage_core::changelog_cache::read() {
            if cache.version == current_version && cache.channel == channel.as_str() {
                return;
            }
        }
        let fetched =
            tauri::async_runtime::spawn_blocking(move || crate::updater::fetch_changelog(channel))
                .await;
        match fetched {
            Ok(Ok(markdown)) => {
                let cache = kage_core::changelog_cache::ChangelogCache {
                    version: current_version.to_string(),
                    channel: channel.as_str().to_string(),
                    fetched_at: chrono::Utc::now().to_rfc3339(),
                    markdown,
                };
                match kage_core::changelog_cache::write(&cache) {
                    Ok(()) => info!("Changelog cache refreshed for {current_version}"),
                    Err(e) => warn!("Failed to write changelog cache: {e}"),
                }
            }
            Ok(Err(e)) => warn!("Changelog cache refresh fetch failed: {e}"),
            Err(e) => warn!("Changelog cache refresh task failed: {e}"),
        }
    });
}

/// Window close-requested handler: hide rather than close, so the app
/// persists in the tray. Logs (rather than panics) if hide fails.
/// On macOS, also hides the app to return focus to the previous application.
pub fn handle_window_close(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    if let Err(e) = window.hide() {
        log::warn!("Failed to hide window on close: {}", e);
    }
    api.prevent_close();

    // Hidden chat window counts as closed for the agent-shutdown
    // decision. Schedule the check; if the user reopens within the
    // grace window (e.g. by clicking the tray) we cancel.
    let label = window.label();
    if is_session_host_label(label) {
        crate::commands::window::schedule_chat_shutdown_check_public(window.app_handle());
    }

    // On macOS: update activation policy (exclude the closing window since
    // is_visible() may not reflect the hide yet), then hide the app to
    // deactivate and return focus to the previous application.
    #[cfg(target_os = "macos")]
    {
        let closing_label = window.label().to_string();
        update_activation_policy_excluding(window.app_handle(), Some(&closing_label));
        hide_macos_app();
    }
}

/// Hide the macOS app (NSApp.hide), returning focus to the previous application.
/// This is the equivalent of Cmd+H — the app stays running but yields focus.
#[cfg(target_os = "macos")]
pub fn hide_macos_app() {
    use objc2::rc::autoreleasepool;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    autoreleasepool(|_pool| {
        // Safe: this is always called from the main thread (UI event handlers).
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            app.hide(None);
        }
    });
}

/// Update the macOS activation policy based on whether any "real" window
/// (chat, settings, store, welcome) is visible. When at least one is
/// visible, switch to Regular (shows in Cmd+Tab and Dock). When none are
/// visible, switch to Accessory (hidden from Cmd+Tab and Dock).
///
/// The floating window is excluded — it's a transient overlay, not
/// something the user Cmd+Tabs to.
///
/// Uses Tauri's built-in `set_activation_policy` which handles main-thread
/// dispatch internally.
#[cfg(target_os = "macos")]
pub fn update_activation_policy(app_handle: &AppHandle) {
    update_activation_policy_excluding(app_handle, None);
}

/// Same as `update_activation_policy` but allows excluding a window label
/// from the visibility check (used when a window is being hidden but
/// `is_visible()` hasn't caught up yet).
#[cfg(target_os = "macos")]
pub fn update_activation_policy_excluding(app_handle: &AppHandle, exclude: Option<&str>) {
    use tauri::ActivationPolicy;

    // Windows that count as "real" for Cmd+Tab purposes
    let real_windows = [
        window_labels::MAIN,
        window_labels::SETTINGS,
        window_labels::STORE,
        window_labels::WELCOME,
    ];

    let any_visible = real_windows.iter().any(|label| {
        if exclude == Some(*label) {
            return false;
        }
        app_handle
            .get_webview_window(label)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false)
    });

    let desired = if any_visible {
        ActivationPolicy::Regular
    } else {
        ActivationPolicy::Accessory
    };

    log::debug!(
        "update_activation_policy: any_visible={}, setting {}",
        any_visible,
        if any_visible { "Regular" } else { "Accessory" }
    );

    if let Err(e) = app_handle.set_activation_policy(desired) {
        log::warn!("Failed to set activation policy: {}", e);
    } else {
        log::debug!(
            "Activation policy set → {}",
            if any_visible { "Regular" } else { "Accessory" }
        );
        // macOS quirk: switching from Accessory → Regular doesn't update
        // the Cmd+Tab list until the app goes through an activation cycle.
        // We must explicitly activate the app after the policy change.
        if any_visible {
            let _ = app_handle.run_on_main_thread(|| {
                use objc2::MainThreadMarker;
                use objc2_app_kit::NSApplication;

                let mtm = unsafe { MainThreadMarker::new_unchecked() };
                let ns_app = NSApplication::sharedApplication(mtm);
                #[allow(deprecated)]
                ns_app.activateIgnoringOtherApps(true);
            });
        }
    }
}

/// No-op on non-macOS platforms.
#[cfg(not(target_os = "macos"))]
pub fn update_activation_policy(_app_handle: &AppHandle) {}

// `update_activation_policy_excluding` has no non-macOS stub on purpose:
// its only caller is the `#[cfg(target_os = "macos")]` block in
// `handle_window_close`, so a cross-platform stub would be dead code.

/// Show the welcome window on first run. Small delay so the floating
/// window has finished initializing before the welcome stacks on top.
pub fn maybe_show_welcome_window(app_handle: &AppHandle, first_run_completed: bool) {
    if first_run_completed {
        return;
    }
    info!("First run detected, showing welcome window");
    let app_handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let _ = crate::commands::system::open_welcome_window(app_handle).await;
    });
}

mod startup;

pub use startup::{maybe_show_floating_after_interactive_install, maybe_spawn_default_session};

/// Kick off the auto-update background loop.
pub fn start_updater(app: &App) {
    let acp: tauri::State<'_, AcpHandles> = app.state();
    let features: tauri::State<'_, FeatureServices> = app.state();
    let ui: tauri::State<'_, UiState> = app.state();
    crate::updater::start_update_loop(
        features.updater.clone(),
        features.config.clone(),
        app.handle().clone(),
        ui.window_sessions.clone(),
        acp.client.clone(),
    );
}

/// Watch the sessions directory for external changes (e.g., the agent
/// backend creating sessions outside of this process). Returns the
/// watcher handle so the caller can drop it on shutdown — that
/// unsubscribes from the platform FS notification and cleans up the
/// background thread cleanly.
pub fn start_session_watcher(app: &App) -> Option<crate::commands::sessions::SessionWatcherHandle> {
    let features: tauri::State<'_, FeatureServices> = app.state();
    crate::commands::sessions::start_session_watcher(
        features.session_cache.clone(),
        app.handle().clone(),
    )
}

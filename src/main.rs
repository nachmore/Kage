// Hide console window on Windows
#![cfg_attr(windows, windows_subsystem = "windows")]

// Shared foundation from the kage-core workspace crate; imported at the
// crate root so existing `crate::lock_ext::...` paths keep resolving.
use kage_core::lock_ext;

mod acp_client;
mod activity_tracker;
mod agent_commands;
mod agent_presets;
mod agent_sessions;
mod app_launcher;
mod app_log;
#[path = "main/app_setup.rs"]
mod app_setup;
#[path = "main/app_startup.rs"]
mod app_startup;
mod auto_steering;
mod automation;
mod chunk_batcher;
mod commands;
mod config;
mod config_export;
mod config_migrations;
#[allow(dead_code)] // Used in lib.rs; main.rs wires the IPC commands in the next commit.
mod context_rules;
mod crash_recovery;
mod ephemeral_session;
mod error;
mod event_targets;
mod events;
mod extensions;
mod hotkey_norm;
mod i18n;
mod link_metadata_cache;
mod logger;
mod mcp_registration;
mod ollama;
mod os;
mod panic_handler;
mod permission_audit;
mod process_manager;
#[path = "main/run_events.rs"]
mod run_events;
mod session_titler;
mod setup;
mod slash_format;
mod startup;
mod state;
mod steering_io;
mod telemetry;
mod tray;
mod updater;
mod webview_recovery;
mod window_labels;

use acp_client::AcpClient;
use config::Config;
use log::{info, warn};
use std::sync::Arc;
use tauri::Manager;

/// On Windows, attach to the parent console (if any) so that logs
/// appear when launched from a terminal. If launched from
/// Explorer/GUI/tray, AttachConsole fails silently and no console is
/// shown.
///
/// Audience matches `logger::init_logger`'s trace gate: `cargo run`
/// debug builds, locally-built dev installers
/// (`KAGE_LOCAL_DEV_BUILD` set by `build_dev_installer.*`), and CI's
/// nightly channel (version contains `+dev.`). Stable/beta release
/// binaries skip the attach so they stay clean.
#[cfg(windows)]
fn attach_parent_console() {
    let allowed = cfg!(debug_assertions)
        || option_env!("KAGE_LOCAL_DEV_BUILD").is_some()
        || env!("CARGO_PKG_VERSION").contains("+dev.");
    if !allowed {
        return;
    }
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() {
    // Handle /capture-hotkey subcommand (helper process mode).
    // This is a Windows-only CLI dispatch path — the helper *is* this
    // very binary re-spawned to run a low-level keyboard hook outside
    // WebView2's input registration. Reaching directly into the
    // platform module is intentional here: it's not a runtime API,
    // it's a CLI mode the rest of the app never touches.
    //
    // Runs before `run()` so the helper mode doesn't pay for the Tokio
    // runtime startup, Tauri builder, panic handler, or any of the
    // normal app scaffolding. It's essentially a different program
    // that happens to share a binary.
    #[cfg(target_os = "windows")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(timeout) = startup::detect_capture_hotkey_subcommand(&args) {
            os::windows::hotkey::run_capture_helper(timeout);
            return;
        }
    }

    // Backtrace env vars must be set HERE — before the multi-thread
    // runtime below spawns its workers. `run()` executes inside
    // `block_on`, by which point worker threads already exist;
    // concurrent getenv/setenv is UB on Unix, and RUST_BACKTRACE is read
    // by the panic machinery on any thread. Dev-mode detection is
    // re-done properly by CliFlags::parse inside run(); this early pass
    // only mirrors its /dev detection for the env-var write.
    {
        let args: Vec<String> = std::env::args().collect();
        if startup::CliFlags::parse(&args).dev_mode {
            if std::env::var_os("RUST_BACKTRACE").is_none() {
                std::env::set_var("RUST_BACKTRACE", "1");
            }
            if std::env::var_os("RUST_LIB_BACKTRACE").is_none() {
                std::env::set_var("RUST_LIB_BACKTRACE", "1");
            }
        }
    }

    // Everything else runs inside a Tokio runtime because:
    //   1. `tauri-plugin-aptabase` calls `tokio::spawn` from its setup
    //      hook (client.rs `start_polling`). Without an ambient runtime
    //      the plugin panics with "no reactor running" before Tauri
    //      even finishes initialising.
    //   2. A handful of our own commands (`get_calendar_events`,
    //      `search_files`, etc.) already depend on `tokio::async_runtime`
    //      being available.
    //
    // We do this via a plain `Runtime::new()` + `block_on` rather than
    // `#[tokio::main]` so the Windows helper-process dispatch above
    // stays in a sync context — it returns before we ever construct a
    // runtime, which keeps the cheap path cheap.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to construct Tokio runtime for main app");
    rt.block_on(run());
}

/// Async entry point. Split out from `main` so the helper-process
/// subcommand can return before we spin up Tokio. See `fn main` for the
/// rationale.
async fn run() {
    #[cfg(windows)]
    attach_parent_console();

    let app_startup::Context {
        args,
        dev_mode,
        debug_mode,
        started_at: startup_t0,
    } = app_startup::initialize();

    // Capture the parsed args once — `main` references `flags` and the
    // setup closure needs the raw argv to resolve the resume marker.
    let main_args = args.clone();

    // Register state on the builder before windows can issue Tauri invokes.
    // Setup retains only work requiring `&mut App` or `app.handle()`.
    let config = startup::load_config_with_overrides(debug_mode, Config::load);
    info!("Configuration loaded");

    // Initialize the localisation catalogs from the embedded JSON resources.
    // Resolution order: explicit override in config.ui.language → OS locale
    // (sys_locale::get_locale, which honours CFLocale / GetUserDefaultLocaleName
    // / $LANG depending on platform) → fall back to English. The function
    // also strips region tags ("en-GB" → "en") when no exact catalog ships.
    let preferred_lang = config.ui.language.clone().or_else(sys_locale::get_locale);
    let active_lang = i18n::init(preferred_lang.as_deref());
    info!("i18n: active language = {}", active_lang);
    if dev_mode {
        info!("⏱ Config loaded at +{}ms", startup_t0.elapsed().as_millis());
    }

    // Initialize the app log ring buffer so frontend `app_log_write`
    // invokes from window startup land on disk. The pre-init buffer in
    // `app_log` catches anything that arrives before this call.
    if let Err(e) = app_log::init(config.system.log_buffer_size) {
        warn!("Failed to initialize app log: {}", e);
    } else {
        app_log::log("info", "system", "App log initialized");
    }

    let active_mode = config.acp.active_mode();
    let (acp_connection_mode, acp_mode_desc) = startup::acp_mode_for(&active_mode);
    info!("{}", acp_mode_desc);
    let acp_client = AcpClient::new(acp_connection_mode);
    acp_client.set_debug_mode(config.debug_mode);

    let process_manager = acp_client.get_process_manager();
    process_manager::install_signal_handlers(process_manager);

    // AppLauncher::new (pure; registry scan deferred to background) is
    // constructed inside build_managed_state below.
    info!("App launcher initialized (scan deferred to background)");
    if dev_mode {
        info!(
            "⏱ App launcher ready at +{}ms",
            startup_t0.elapsed().as_millis()
        );
    }

    let acp_client_arc = Arc::new(acp_client);
    let config_arc = Arc::new(std::sync::Mutex::new(config.clone()));

    // All Tauri-managed state comes from one constructor shared with the
    // mock-app test harness, so tests manage exactly what production does.
    let state::ManagedState {
        acp_handles,
        ui_state,
        child_processes,
        feature_services,
    } = state::build_managed_state(config_arc.clone(), acp_client_arc.clone(), dev_mode);

    // Clone Arcs for the notification handler setup (wired up inside
    // setup() because it needs app.handle() to emit Tauri events).
    let config_for_handler = config_arc.clone();
    let slash_cmds_for_handler = acp_handles.slash_commands.clone();
    let pending_perm_for_handler = acp_handles.pending_permissions.clone();
    let acp_for_handler = acp_client_arc.clone();
    // Register a signal-handler hook that kills the pocket-tts server
    // and any in-flight pip install. Windows already reaps these via
    // the parent Job Object, but macOS / Linux signal-driven exits
    // (SIGTERM, SIGINT) used to leak them — graceful_shutdown only
    // ran on tray-quit / quit_app paths, not signal paths. This
    // closes the gap for non-Windows platforms; on Windows it's
    // redundant-but-harmless.
    {
        let tts = child_processes.pocket_tts.clone();
        let install = child_processes.pocket_tts_install.clone();
        process_manager::register_child_killer(move || {
            if let Ok(mut slot) = tts.lock() {
                if let Some(mut child) = slot.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            if let Ok(mut slot) = install.lock() {
                if let Some(mut child) = slot.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        });
    }
    // Capture a clone of config for the parts of setup() that still
    // need it (hotkey hot-reload installer, autostart pocket TTS, etc.).
    let config_for_setup = config.clone();

    // Holder for the session-watcher's shutdown handle. Populated in
    // `setup()` once the AppHandle is available; dropped in the
    // RunEvent::Exit branch below to give the watcher's background
    // thread a clean shutdown signal (which in turn drops the FS
    // notification subscription cleanly). Pre-fix the watcher thread
    // sat in `loop { sleep(3600s) }` and was only ever cleaned up by
    // process death.
    let session_watcher_handle: Arc<
        std::sync::Mutex<Option<commands::sessions::SessionWatcherHandle>>,
    > = Arc::new(std::sync::Mutex::new(None));
    let session_watcher_handle_for_setup = session_watcher_handle.clone();

    let mut builder = tauri::Builder::default()
        // Single-instance enforcement. Must be the FIRST plugin registered
        // (per the plugin's docs) so the second-process exit happens before
        // any window-creation work runs in that process. The callback fires
        // in the *primary* process when a second launch happens — we just
        // emit `show-sessions`, which the existing listener routes to
        // `open_chat_window`. Same event name as the previous hand-rolled
        // TCP IPC, so the frontend wiring is unchanged.
        //
        // The expensive startup work (orphan cleanup, ACP subprocess spawn,
        // app registry scan, hotkey registration, tray construction, all
        // the listener installs) still lives in `.setup()` below — the
        // plugin exits the second process before any of that runs. The
        // state construction above is cheap (Config::load is one small
        // file read; AcpClient::new and AppLauncher::new are pure
        // allocations; the rest is `Arc::new(Mutex::new(...))`), so the
        // second-instance cost stays bounded by the plugin's own IPC dance
        // (well under 50ms over named pipes / AF_UNIX).
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            use tauri::Emitter;
            info!("Second instance signaled via single-instance plugin");
            // Suppress the chat-window pop when the second launch was
            // a deep-link click. The intent there is "install this
            // extension," which the deep-link plugin handles
            // independently via `on_open_url` (the `deep-link`
            // feature flag on single-instance forwards the URL into
            // that channel before this callback runs). Showing the
            // chat sessions on top of the store window confused users
            // — the click had nothing to do with sessions.
            //
            // For every other second-instance launch (a user clicking
            // the tray icon's launcher again, double-clicking the exe,
            // etc.) we still want the focus-the-chat behaviour, which
            // is the original raison d'être of this callback.
            let is_deep_link = argv.iter().any(|a| a.starts_with("kage://"));
            if !is_deep_link {
                let _ = app.emit(events::SHOW_SESSIONS, ());
            }
        }))
        // Custom URL scheme handler for `kage://`. Registered after
        // single-instance so the second-instance argv has already been
        // forwarded into our channel. Self-registration on first launch
        // (Windows: HKCU\Software\Classes\kage; macOS: handled by the
        // bundler from CFBundleURLTypes; Linux: writes a .desktop file)
        // happens via `register_all()` in setup.rs — the binary always
        // knows its own path, which beats hand-coding it into NSIS.
        .plugin(tauri_plugin_deep_link::init());

    // Aptabase plugin — only registered when a compile-time key was
    // provided (via APTABASE_KEY env var at build time). Without a key
    // the plugin is never in the process, so no background tasks, no
    // network activity. Registered after single-instance so a second
    // launch short-circuits before aptabase spins up any workers.
    //
    // Region routing is implicit: the plugin reads the second segment
    // of the key (`A-EU-xxxx` → eu.aptabase.com, `A-US-xxxx` →
    // us.aptabase.com, `A-DEV-xxxx` → localhost, `A-SH-xxxx` →
    // self-hosted via InitOptions::host). Our key is EU so the EU
    // endpoint is picked automatically — matches docs/PRIVACY.md.
    //
    // The panic_hook composes with our existing panic_handler::install
    // (the file-based crash.log writer). Order of hooks at panic time:
    // Aptabase fires our `panic` event and flushes, then chains to the
    // crash.log writer, then the rust default. The hook itself reads
    // config from disk to re-check consent at panic time.
    //
    // See src/telemetry.rs and docs/PRIVACY.md.
    if let Some(key) = telemetry::APTABASE_KEY {
        // Log enough to diagnose telemetry outages from app.jsonl alone
        // — region prefix only (key[2..4]) is the part that determines
        // ingest endpoint routing, and it's already public (it's
        // visible in outbound HTTP). Full key is gated on /debug.
        let region = key.split('-').nth(1).unwrap_or("?");
        log::info!(
            "Telemetry: aptabase plugin registered (region={}, key_len={})",
            region,
            key.len()
        );
        // Pin the flush interval to 60s. The plugin's default keys off
        // its own `cfg(debug_assertions)` (2s in debug, 60s in release).
        // For us, a debug-profile build still hits a real network and
        // a real dashboard, so the 2s default produces a "flushing /
        // nothing to send" log line every two seconds with zero
        // diagnostic value — and burns CPU + log space for nothing.
        // 60s matches what end users see on stable / beta releases.
        let init_opts = tauri_plugin_aptabase::InitOptions {
            host: None,
            flush_interval: Some(std::time::Duration::from_secs(60)),
        };
        builder = builder.plugin(
            tauri_plugin_aptabase::Builder::new(key)
                .with_options(init_opts)
                .with_panic_hook(telemetry::panic_hook())
                .build(),
        );
    } else {
        log::info!("Telemetry: aptabase plugin NOT registered (no compile-time APTABASE_KEY)");
    }

    let builder = builder
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // Signed in-app updates. The plugin verifies every downloaded
        // installer against the compile-time public key embedded via
        // build.rs; a missing signature or wrong key aborts the install
        // before any bytes execute. Endpoint + pubkey are configured at
        // runtime in `updater::plugin_check` so we can honour the
        // user's channel choice (stable / beta / dev). See docs/RELEASE.md.
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Register Tauri-managed state at build time, BEFORE any window
        // exists. This is essential: webview JS can fire `tauri::State<…>`
        // -bound invokes the moment the window is constructed, which is
        // before our `.setup()` block runs. Registering inside setup()
        // produced "state not managed" errors for every chat-window-open
        // until ~5s into startup. See the comment block above the state
        // construction (start of `run`) for why this is correctness work,
        // not optimisation.
        .manage(acp_handles)
        .manage(ui_state)
        .manage(child_processes)
        .manage(feature_services)
        .on_window_event(|window, event| {
            // Diagnostic logging for the floating window only — helps us
            // see when something hides/destroys/focuses it externally.
            // Other windows are too noisy (chat repaints, etc.) so we
            // gate on label.
            if window.label() == window_labels::FLOATING {
                match event {
                    tauri::WindowEvent::CloseRequested { .. } => {
                        log::info!("[floating-event] CloseRequested");
                    }
                    tauri::WindowEvent::Destroyed => {
                        log::info!("[floating-event] Destroyed");
                    }
                    tauri::WindowEvent::Focused(focused) => {
                        log::info!("[floating-event] Focused({})", focused);
                    }
                    tauri::WindowEvent::Resized(sz) => {
                        log::info!("[floating-event] Resized(w={}, h={})", sz.width, sz.height);
                    }
                    _ => {}
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                setup::handle_window_close(window, api);
            }
        })
        .on_page_load(|webview, _payload| {
            // Subscribe to WebView2's ProcessFailed event the moment a
            // webview finishes its first navigation. Earliest hook that
            // works for both pre-declared (main/floating/inline-assist)
            // and on-demand (settings/store/welcome/chat-*) windows;
            // setup() only runs once at app boot. Idempotent — the
            // listener tracks installed labels and skips re-registration
            // on subsequent navigations within the same webview.
            // No-op on non-Windows.
            webview_recovery::install_process_failed_for(webview);
        })
        .setup(move |app| {
            app_setup::configure(
                app,
                dev_mode,
                &main_args,
                &config_for_setup,
                acp_for_handler,
                config_for_handler,
                slash_cmds_for_handler,
                pending_perm_for_handler,
                &session_watcher_handle_for_setup,
            )
        });

    let app = builder
        .invoke_handler(commands::registry::invoke_handler())
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    // Record startup events (install / upgrade / daily active) now that
    // FeatureServices (and its Config) are managed. Gated on the user
    // having opted in and the build having a compile-time key.
    if let Some(features) = app.try_state::<state::FeatureServices>() {
        telemetry::record_startup_events(app.handle(), &features.config);
    }

    // Drive the Tauri event loop, flushing telemetry on exit so the
    // final app_exited event actually reaches the server before the
    // process dies. Drop the session-watcher handle on Exit so the
    // background thread sees its shutdown channel disconnect, drops
    // the platform FS subscription, and exits cleanly.
    run_events::run(app, session_watcher_handle);

    info!("Application shutting down");
}

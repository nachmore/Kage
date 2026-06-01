// Hide console window on Windows
#![cfg_attr(windows, windows_subsystem = "windows")]

mod acp_client;
mod activity_tracker;
mod agent_presets;
mod agent_sessions;
mod app_launcher;
mod app_log;
mod auto_steering;
mod automation;
mod chunk_batcher;
mod commands;
#[allow(dead_code)] // Consumed by the kage-computer-control-mcp binary, not this one
mod computer_control;
mod config;
mod config_export;
mod config_migrations;
#[allow(dead_code)] // Used in lib.rs; main.rs wires the IPC commands in the next commit.
mod context_rules;
mod crash_recovery;
mod error;
mod events;
mod extensions;
mod hotkey_norm;
mod i18n;
mod link_metadata_cache;
mod lock_ext;
mod logger;
mod mcp_registration;
mod ollama;
mod os;
mod panic_handler;
mod permission_audit;
mod process_manager;
mod session_titler;
mod setup;
mod startup;
mod state;
mod steering_io;
mod telemetry;
mod tray;
mod updater;
mod webview_recovery;
mod window_labels;

use acp_client::AcpClient;
use app_launcher::AppLauncher;
use config::Config;
use log::{info, warn};
use process_manager::ProcessManager;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::Mutex;

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

    // Install the panic hook as early as possible so any panic during startup
    // still gets captured into crash.log.
    panic_handler::install();

    // Initialize logger first
    if let Err(e) = logger::init_logger() {
        eprintln!("Failed to initialize logger: {}", e);
        eprintln!("Continuing without file logging...");
    }

    // Read the WebView2 recovery marker BEFORE any tauri / wry code can
    // emit a wedge error. The marker tells us whether this run is a
    // restart from a previous wedge — so a second wedge in the new
    // process knows to escalate (delay-then-restart) instead of
    // restarting again immediately.
    webview_recovery::init_at_startup();

    info!("=== Kage Starting ===");
    let startup_t0 = std::time::Instant::now();

    let args: Vec<String> = std::env::args().collect();
    let flags = startup::CliFlags::parse(&args);

    // Single-instance enforcement is wired in further below as a Tauri
    // plugin (tauri-plugin-single-instance). The plugin's setup hook
    // detects an existing primary, forwards argv/cwd over a platform-
    // appropriate IPC (named pipe on Windows, AF_UNIX socket on Unix —
    // both have OS-enforced user-bound access control, unlike the prior
    // loopback-TCP IPC), and exits the second process before window
    // creation.
    //
    // On restart, wait for the old process to fully release WebView2/Tauri
    // resources before we proceed.
    startup::wait_for_previous_instance_if_restart(flags.is_restart);

    // Independently of the restart flow, every launch verifies the
    // WebView2 user data folder is writable. If a previous kage was
    // force-killed, its msedgewebview2 children may still be holding
    // the directory lock — that surfaces as the "Frontend did not
    // become ready" timeout 15 seconds into startup. This call kills
    // those orphans before Tauri tries to attach. No-op on macOS/Linux.
    startup::ensure_webview_directory_writable();

    let dev_mode = flags.dev_mode;
    let debug_mode = flags.debug_mode;

    // In dev mode, enable Rust backtraces on panic unless the user has
    // already set RUST_BACKTRACE explicitly (e.g. to "full"). This means
    // `cargo tauri dev -- /dev` always produces useful panic traces.
    // RUST_LIB_BACKTRACE controls backtraces captured by std::backtrace
    // on Error types (e.g. anyhow), as opposed to panics.
    if dev_mode {
        // SAFETY: called before any threads are spawned that read these vars.
        if std::env::var_os("RUST_BACKTRACE").is_none() {
            std::env::set_var("RUST_BACKTRACE", "1");
        }
        if std::env::var_os("RUST_LIB_BACKTRACE").is_none() {
            std::env::set_var("RUST_LIB_BACKTRACE", "1");
        }
    }

    if debug_mode {
        println!("🐛 DEBUG MODE ENABLED - Detailed ACP logs will be printed to console");
        info!("🐛 DEBUG MODE ENABLED via command line argument");
        logger::enable_console_logging();
    }

    if dev_mode {
        info!(
            "⏱ Tauri builder starting at +{}ms",
            startup_t0.elapsed().as_millis()
        );
    }

    // Capture the parsed args once — `main` references `flags` and the
    // setup closure needs the raw argv to resolve the resume marker.
    let main_args = args.clone();

    // Construct shared state BEFORE Tauri's builder so we can `.manage()`
    // it on the builder itself. The motivation is correctness, not speed:
    // Tauri loads each window's HTML/JS as soon as the builder constructs
    // it, and the webview's `main.js` starts firing `invoke('get_config')`
    // / `invoke('get_user_info')` / etc. several seconds before our
    // `.setup()` block runs. If state is registered inside `setup()`, those
    // early invokes return "state not managed for field 'features' on
    // command 'get_config'. You must call `.manage()` before using this
    // command" — and the chat window initialises against an empty backend.
    //
    // What stays in setup(): work that needs `&mut App` / `app.handle()`
    // (notification handler wiring, tray construction, hotkey registration,
    // window decoration, background tasks). Cheap object construction
    // moves out here. A second instance now pays for these allocations
    // before the single-instance plugin's setup hook exits the process,
    // but the cost is bounded — `Config::load` is one small file read,
    // `AcpClient::new` and `AppLauncher::new` are pure allocations
    // (subprocess spawn / registry scan are deferred). The 50ms-ish
    // single-instance IPC dance still dominates a second-instance launch.
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

    let app_launcher = AppLauncher::new();
    info!("App launcher initialized (scan deferred to background)");
    if dev_mode {
        info!(
            "⏱ App launcher ready at +{}ms",
            startup_t0.elapsed().as_millis()
        );
    }

    let acp_client_arc = Arc::new(acp_client);
    let config_arc = Arc::new(std::sync::Mutex::new(config.clone()));
    let slash_commands_arc = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pending_permission_arc = Arc::new(std::sync::Mutex::new(None));
    let available_models_arc =
        Arc::new(std::sync::Mutex::new(Vec::<crate::state::AcpModel>::new()));

    // Clone Arcs for the notification handler setup (wired up inside
    // setup() because it needs app.handle() to emit Tauri events).
    let config_for_handler = config_arc.clone();
    let slash_cmds_for_handler = slash_commands_arc.clone();
    let pending_perm_for_handler = pending_permission_arc.clone();
    let acp_for_handler = acp_client_arc.clone();

    let acp_handles = state::AcpHandles {
        client: acp_client_arc,
        pending_permission: pending_permission_arc,
        slash_commands: slash_commands_arc,
        available_models: available_models_arc,
        last_tool_steering_hash: Arc::new(std::sync::Mutex::new(0)),
    };
    let ui_state = state::UiState {
        dev_mode,
        window_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        pending_prompt_originators: Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        last_focused_chat: Arc::new(std::sync::Mutex::new(None)),
        chat_shutdown_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        last_selection: Arc::new(std::sync::Mutex::new(None)),
        source_window: Arc::new(std::sync::Mutex::new(None)),
        frontend_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let child_processes = state::ChildProcesses {
        pocket_tts: Arc::new(std::sync::Mutex::new(None)),
        pocket_tts_install: Arc::new(std::sync::Mutex::new(None)),
    };
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
    let feature_services = state::FeatureServices {
        config: config_arc,
        app_launcher: Arc::new(Mutex::new(app_launcher)),
        updater: Arc::new(updater::UpdaterState::new()),
        user_info_cache: Arc::new(std::sync::Mutex::new(None)),
        session_cache: Arc::new(std::sync::Mutex::new(None)),
        automation_plan_cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        activity_tracker: Arc::new(crate::activity_tracker::ActivityTrackerState::new()),
        agent_session_registry: Arc::new(crate::agent_sessions::AgentSessionRegistry::new()),
        automation_signal_tx: Arc::new(std::sync::Mutex::new(None)),
    };

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

    let app = builder
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
        .setup(move |app| {
            info!("Setting up application");
            info!("=== Kage Setup ===");

            // macOS: override the default app menu so Cmd+Q hides windows
            // instead of quitting. Users quit via the tray menu's "Quit".
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
                use tauri::Manager;

                let hide_item = MenuItemBuilder::with_id("macos-hide", "Hide Kage")
                    .accelerator("CmdOrCtrl+Q")
                    .build(app)?;
                let hide_others = MenuItemBuilder::with_id("macos-hide-others", "Hide Others")
                    .accelerator("CmdOrCtrl+Alt+H")
                    .build(app)?;
                let show_all = MenuItemBuilder::with_id("macos-show-all", "Show All").build(app)?;
                let quit_item = MenuItemBuilder::with_id("macos-quit", "Quit Kage")
                    .accelerator("CmdOrCtrl+Shift+Q")
                    .build(app)?;

                let app_submenu = SubmenuBuilder::new(app, "Kage")
                    .items(&[&hide_item, &hide_others, &show_all])
                    .separator()
                    .item(&quit_item)
                    .build()?;

                let menu = MenuBuilder::new(app).item(&app_submenu).build()?;
                app.set_menu(menu)?;

                let app_handle = app.handle().clone();
                app.on_menu_event(move |_app, event| match event.id().as_ref() {
                    "macos-hide" => {
                        info!("Cmd+Q: hiding all windows (use tray Quit to exit)");
                        for (_, window) in _app.webview_windows() {
                            let _ = window.hide();
                        }
                        setup::hide_macos_app();
                        setup::update_activation_policy(_app);
                    }
                    "macos-hide-others" => {
                        // NSApp hideOtherApplications — not directly available,
                        // but hiding our windows achieves the user intent.
                        for (_, window) in _app.webview_windows() {
                            let _ = window.hide();
                        }
                        setup::hide_macos_app();
                        setup::update_activation_policy(_app);
                    }
                    "macos-show-all" => {
                        if let Some(w) = _app.get_webview_window(window_labels::FLOATING) {
                            let _ = w.show();
                        }
                    }
                    "macos-quit" => {
                        info!("Cmd+Shift+Q: quitting application");
                        crate::commands::system::graceful_shutdown(&app_handle);
                        let app_for_exit = app_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            crate::commands::system::shutdown_and_exit(&app_for_exit).await;
                        });
                    }
                    _ => {}
                });
            }

            // macOS: start as Accessory (no Cmd+Tab / Dock) since only the
            // floating window is visible at launch. LSUIElement in Info.plist
            // handles this for release builds, but during `cargo tauri dev`
            // the plist isn't bundled — so we set it programmatically too.
            #[cfg(target_os = "macos")]
            {
                use tauri::ActivationPolicy;
                if let Err(e) = app
                    .handle()
                    .set_activation_policy(ActivationPolicy::Accessory)
                {
                    warn!("Failed to set initial activation policy: {}", e);
                } else {
                    info!("Set initial activation policy to Accessory");
                }
            }

            // Check for session resume after update. The marker file (or
            // /resume-session CLI arg) is *always* consumed here so a stale
            // marker doesn't ghost-trigger on the next normal launch.
            let resume_session_id: Option<String> = dirs::config_dir()
                .map(|d| d.join("kage"))
                .and_then(|cfg_dir| startup::resolve_resume_session_id(&main_args, &cfg_dir));
            if let Some(ref id) = resume_session_id {
                info!("Resume marker present: will attempt to load session {}", id);
            }

            // Background orphan cleanup. Runs in the primary only — a second
            // instance must never trigger this because it could read the
            // primary's PID and try to kill it (the recycled-PID guard helps
            // but isn't proof). Scoped to the primary by living in setup().
            info!("Checking for orphaned processes...");
            std::thread::spawn(|| {
                if let Err(e) = ProcessManager::cleanup_orphaned_processes() {
                    warn!("Failed to cleanup orphaned processes: {}", e);
                }
            });

            // Create a Job Object that auto-kills all child processes when this
            // process exits, even on crash — prevents orphaned TTS servers, ACP
            // CLI processes, MCP children, etc. No-op on non-Windows where
            // orphan reaping happens via init/launchd.
            os::install_kill_on_exit_job();

            // Config, app_log, AcpClient, AppLauncher, and all Tauri-managed
            // state were constructed before `tauri::Builder::default()` so the
            // state is registered on the Builder itself — see the comment
            // block at the top of `run`. Setup() picks up where that leaves
            // off: the work below needs `&mut App` / `app.handle()`.

            // Build system tray
            tray::setup_tray(app, dev_mode)?;

            // Set up the ACP notification handler. The handler captures an
            // Arc<AcpClient> so it can issue protocol replies (permission
            // responses, etc.) through the typed client API instead of
            // hand-building JSON-RPC out-of-band.
            commands::messaging::setup_notification_handler(
                acp_for_handler,
                app.handle(),
                config_for_handler,
                slash_cmds_for_handler,
                pending_perm_for_handler,
            );

            // Configure floating / context-menu / inline-assist windows for transparency.
            setup::configure_transparent_windows(app);

            // Register all global hotkeys from config
            commands::system::register_all_hotkeys(app.handle());

            // Hot-reload hotkeys when config changes
            setup::install_hotkey_hot_reload(app, &config_for_setup);

            info!("=== Setup Complete ===");

            // Listen for show-sessions event. Emitted by the
            // single-instance plugin's callback when a second process
            // launches; routed to the most-recently focused chat window.
            setup::install_show_sessions_listener(app);

            // Self-register the kage:// URL scheme with the OS and
            // listen for incoming kage://install/<id> deep links. Both
            // the cold-launch case (`get_current()`) and the warm-launch
            // case (`on_open_url`) route through the same handler.
            setup::install_deep_link_handler(app);

            // Track focus on main so single-instance / show-sessions
            // routing has an accurate "most recent" target. Peer chat
            // windows install their own listener at construction.
            setup::install_main_focus_tracker(app);

            // Start automation scheduler
            setup::spawn_automation_scheduler(app);

            // Auto-start Pocket TTS server if configured
            setup::maybe_autostart_pocket_tts(app, &config_for_setup);

            // Watch the sessions directory for external changes (e.g., the agent backend creating sessions).
            // The returned handle owns the shutdown signal — stash it in the
            // process-wide holder so RunEvent::Exit can drop it cleanly.
            if let Some(h) = setup::start_session_watcher(app) {
                if let Ok(mut slot) = session_watcher_handle_for_setup.lock() {
                    *slot = Some(h);
                }
            }

            // Self-heal computer-control MCP registration: keep the
            // path in mcp.json in sync with the current install, and
            // log loudly if the sidecar binary is missing entirely.
            setup::refresh_mcp_registration_if_enabled();

            // Background app registry scan (deferred from startup for speed)
            // and periodic refresh every hour so the list stays current.
            setup::spawn_app_registry_scan(app);

            // Start default session on launch if configured. Pass through
            // the resume id consumed at startup so the post-update launch
            // restores the user's session instead of silently dropping it.
            setup::maybe_spawn_default_session(app, &config_for_setup, resume_session_id);

            // Start the auto-update background loop
            setup::start_updater(app);

            // Show welcome window on first run
            setup::maybe_show_welcome_window(app.handle(), config_for_setup.first_run_completed);

            // If the previous run was a user-initiated install, show the
            // floating window now so the user gets immediate visual
            // feedback that their click completed. Idle installs leave
            // the window hidden — banner shows next time the user
            // summons it manually.
            setup::maybe_show_floating_after_interactive_install(app.handle());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_i18n_catalog,
            commands::get_available_languages,
            commands::set_language,
            commands::read_extension_locale,
            commands::send_message_streaming,
            commands::check_connection,
            commands::open_chat_with_message,
            commands::get_config,
            commands::save_config,
            commands::open_settings_window,
            commands::reconnect_acp,
            commands::handle_floating_input,
            commands::launch_app_by_name,
            commands::open_url,
            commands::fetch_link_metadata,
            commands::link_metadata_clear_cache,
            commands::link_metadata_cache_stats,
            commands::open_path,
            commands::execute_shortcut,
            commands::test_floating_window,
            commands::start_drag_window,
            commands::open_chat_window,
            commands::open_new_chat_window,
            commands::close_chat_window,
            commands::list_chat_windows,
            commands::resize_floating_window,
            commands::send_permission_response,
            commands::remove_tool_permission,
            commands::update_tool_policy,
            commands::get_permission_audit_log,
            commands::clear_permission_audit_log,
            commands::get_permission_audit_log_path,
            commands::is_dev_mode,
            commands::is_terminator_mode,
            commands::open_devtools,
            commands::capture_hotkey_combo,
            commands::cancel_hotkey_capture,
            commands::try_register_hotkey,
            commands::get_app_info,
            commands::get_os_dark_mode,
            commands::open_welcome_window,
            commands::complete_first_run,
            commands::trigger_welcome_banner,
            commands::is_first_run,
            commands::detect_agents,
            commands::list_agent_presets,
            commands::validate_agent_connection,
            commands::probe_connection_version,
            commands::check_npm_available,
            commands::install_acp_wrapper,
            commands::get_startup_enabled,
            commands::set_startup_enabled,
            commands::get_computer_control_enabled,
            commands::set_computer_control_enabled,
            commands::get_mcp_json_path,
            commands::get_mcp_config,
            commands::save_mcp_config,
            commands::agent_session_providers,
            commands::agent_list_sessions,
            commands::agent_load_session,
            commands::agent_check_session_updated,
            commands::kage_desktop_workspaces,
            commands::kage_desktop_delete_session,
            commands::kage_desktop_open_folder,
            commands::quit_app,
            commands::restart_app,
            commands::read_clipboard,
            commands::resolve_directories,
            commands::get_clipboard_history,
            commands::paste_clipboard_item,
            commands::fetch_favicon,
            commands::record_shortcut_usage,
            commands::get_shortcut_history,
            commands::search_files,
            commands::get_calendar_events,
            commands::get_calendar_events_for_date,
            commands::show_context_menu,
            commands::set_floating_opacity,
            commands::apply_chat_window_size,
            commands::save_window_position,
            commands::save_chat_window_geometry,
            commands::get_last_selection,
            commands::get_user_info,
            commands::list_sessions,
            commands::load_session,
            commands::switch_acp_session,
            commands::rename_session,
            commands::reveal_session_file,
            commands::get_sessions_directory,
            commands::delete_session,
            commands::get_window_session,
            commands::set_window_session,
            commands::clear_window_session,
            commands::get_steering_content,
            commands::open_auto_steering_file,
            commands::get_auto_steering_path,
            commands::read_steering_lines,
            commands::write_steering_lines,
            commands::import_steering_lines,
            commands::match_context_rule,
            commands::ollama_probe,
            commands::ollama_list_models,
            commands::ollama_codex_spawn_command,
            commands::export_config_default_filename,
            commands::export_config_bundle,
            commands::import_config_bundle,
            commands::write_text_file,
            commands::get_recent_crash,
            commands::dismiss_recent_crash,
            commands::send_steering_message,
            commands::dismiss_pending_permission,
            commands::has_pending_permission,
            commands::get_slash_commands,
            commands::execute_slash_command,
            commands::get_slash_command_options,
            commands::get_available_models,
            commands::check_for_update,
            commands::fetch_changelog,
            commands::get_update_urls,
            commands::download_and_install_update,
            commands::was_just_updated,
            commands::clear_update_flag,
            commands::touch_floating_activity,
            commands::execute_system_command,
            commands::cancel_generation,
            commands::save_frecency,
            commands::load_frecency,
            commands::list_extensions,
            commands::list_themes,
            commands::list_command_packs,
            commands::get_extension_config,
            commands::save_extension_config,
            commands::set_extension_enabled,
            commands::load_theme_colors,
            commands::install_extension_from_path,
            commands::uninstall_extension,
            commands::remove_extension_grant,
            commands::commit_extension_install,
            commands::open_store_window,
            commands::store_get_catalog,
            commands::store_get_detail,
            commands::store_install,
            commands::welcome_provision_extensions,
            commands::check_extension_updates,
            commands::read_extension_file,
            commands::save_store_url,
            commands::save_extension_data,
            commands::load_extension_data,
            commands::delete_extension_data,
            commands::oauth_loopback_start,
            commands::oauth_loopback_await,
            commands::oauth_loopback_cancel,
            commands::pocket_tts_check_install,
            commands::pocket_tts_install,
            commands::pocket_tts_cancel_install,
            commands::pocket_tts_start,
            commands::pocket_tts_stop,
            commands::pocket_tts_voices,
            commands::pocket_tts_test,
            commands::execute_automation_plan,
            commands::extension_tool_response,
            commands::send_extension_tool_steering,
            commands::check_extension_tool_permission,
            commands::pick_folder,
            commands::scan_folder,
            commands::execute_folder_plan,
            commands::get_common_folders,
            commands::notify_frontend_ready,
            commands::list_open_windows,
            commands::get_window_icons,
            commands::get_process_name,
            commands::focus_open_window,
            crate::automation::emit_automation_signal,
            crate::automation::get_power_status,
            crate::automation::list_automation_signals,
            commands::start_activity_tracker,
            commands::stop_activity_tracker,
            commands::get_activity_report,
            commands::is_activity_tracker_running,
            commands::get_app_icon,
            commands::get_source_window,
            commands::get_screen_context,
            commands::show_inline_assist,
            commands::inline_assist_apply,
            commands::send_inline_assist,
            commands::execute_macro,
            commands::app_log_write,
            commands::app_log_get_entries,
            commands::app_log_clear,
            commands::app_log_get_dir,
            commands::dump_thread_info,
            commands::get_telemetry_info,
            commands::set_telemetry_enabled,
            commands::reset_telemetry_install_id,
            commands::telemetry_track,
        ])
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
    app.run(move |handler, event| {
        if let tauri::RunEvent::Exit = event {
            if let Ok(mut slot) = session_watcher_handle.lock() {
                slot.take();
            }
            telemetry::record_shutdown(handler);
        }
    });

    info!("Application shutting down");
}

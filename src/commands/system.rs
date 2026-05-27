use crate::config::Config;
use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::os;
use crate::state::{AcpHandles, ChildProcesses, FeatureServices, UiState};
use log::{error, info, warn};
use std::fs;
use tauri::{Emitter, Manager, State};

/// Re-export steering constants from auto_steering (the canonical location).
pub use crate::auto_steering::{BUILTIN_STEERING, STEERING_MSG_PREFIX};

/// Consolidated shutdown: hide UI, kill TTS, generate steering, disconnect ACP.
/// Called from tray quit, quit_app, and restart_app to avoid duplicated cleanup.
pub fn graceful_shutdown(app: &tauri::AppHandle) {
    // Hide all windows and tray for instant visual feedback
    for label in &["floating", "main", "settings", "context-menu"] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_visible(false);
    }

    // Kill pocket-tts server and any in-flight pip install if running.
    // The Job Object reaps both on Windows when we exit, but macOS/Linux
    // have no equivalent — without an explicit kill here, a Cmd+Q during
    // a Pocket TTS install leaves the install running headless.
    if let Some(procs) = app.try_state::<ChildProcesses>() {
        let mut tts_proc = procs.pocket_tts.lock_or_recover();
        if let Some(mut child) = tts_proc.take() {
            info!("Stopping pocket-tts server on shutdown");
            let _ = child.kill();
            let _ = child.wait();
        }
        let mut install_proc = procs.pocket_tts_install.lock_or_recover();
        if let Some(mut child) = install_proc.take() {
            info!("Cancelling in-flight pocket-tts install on shutdown");
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    // Flush app log to disk
    crate::app_log::flush();
}

/// Run the async portion of shutdown (steering + disconnect) then exit.
/// Must be called from an async context after `graceful_shutdown`.
pub async fn shutdown_and_exit(app: &tauri::AppHandle) {
    shutdown_and_exit_inner(app, None).await;
}

/// Shutdown with optional restart: spawns a new process right before exit.
pub async fn shutdown_and_exit_with_restart(
    app: &tauri::AppHandle,
    exe: std::path::PathBuf,
    args: Vec<String>,
) {
    shutdown_and_exit_inner(app, Some((exe, args))).await;
}

async fn shutdown_and_exit_inner(
    app: &tauri::AppHandle,
    restart: Option<(std::path::PathBuf, Vec<String>)>,
) {
    if let (Some(acp), Some(features)) = (
        app.try_state::<AcpHandles>(),
        app.try_state::<FeatureServices>(),
    ) {
        let acp_client = acp.client.clone();
        let config = features.config.clone();

        // Quit-time steering operates on main's pinned session — that's
        // the conventional "primary" conversation and the one users
        // expect to be analysed. Floating sessions are typically short
        // and transactional; chat-<uuid> windows aren't yet ubiquitous
        // enough to be worth picking between. If main has no pinned
        // session, skip steering entirely.
        let main_session_id = app.try_state::<crate::state::UiState>().and_then(|ui| {
            ui.window_sessions
                .lock()
                .ok()
                .and_then(|m| m.get("main").cloned())
        });

        // Snapshot whether auto-steering will actually run before we pay any
        // cancel-and-wait cost. If there's nothing to steer, quit can skip
        // straight to disconnect.
        let will_run_steering = main_session_id.is_some()
            && if let Ok(cfg) = config.try_lock() {
                cfg.acp.agent.auto_steering_enabled
                    && acp_client.is_connected()
                    && crate::auto_steering::has_pending_messages()
            } else {
                false
            };

        if will_run_steering {
            let session_id = main_session_id.expect("checked above");
            // Cancel any in-flight prompt on this session before
            // issuing the steering prompt. The agent (kiro-cli) holds
            // an internal "prompt in progress" lock per session that
            // rejects any subsequent session/prompt until the active
            // prompt finishes or is cancelled. Without this, on-quit
            // steering races with the user's last prompt and the
            // agent replies with "Prompt already in progress" instead
            // of generating the doc.
            if let Err(e) = acp_client.cancel_session(&session_id) {
                warn!("Failed to send session/cancel on quit: {}", e);
            } else {
                // Brief pause so the agent can release its prompt
                // lock before auto_steering issues a new prompt.
                // 150ms is plenty for a local stdio agent; kept
                // small so quit still feels instant.
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }

            if let Ok(config) = config.try_lock() {
                crate::auto_steering::generate_steering_on_quit(&acp_client, &config, &session_id);
            }
        }

        acp_client.disconnect();
    }

    // Spawn new instance right before exit (if restarting). On Windows we
    // pass CREATE_BREAKAWAY_FROM_JOB via `os::configure_breakaway_from_job`
    // so the new instance isn't tied to the dying parent's Job Object — see
    // `os::install_kill_on_exit_job`. The helper is a no-op on macOS/Linux,
    // where parent-exit reaping is handled by init/launchd.
    if let Some((exe, args)) = restart {
        info!("Spawning restart: {:?} {:?}", exe, args);
        let mut restart_cmd = std::process::Command::new(&exe);
        restart_cmd
            .args(&args)
            .current_dir(std::env::current_dir().unwrap_or_default());
        crate::os::configure_breakaway_from_job(&mut restart_cmd);
        match restart_cmd.spawn() {
            Ok(child) => info!("Restart process spawned (PID: {})", child.id()),
            Err(e) => error!("Failed to spawn restart process: {}", e),
        }
    }

    std::process::exit(0);
}

/// Assemble the full steering content from builtin + user + auto sources.
/// Returns the joined parts (without the STEERING_MSG_PREFIX wrapper).
/// Callers are responsible for adding the prefix and any instructions.
pub fn assemble_steering_parts(config: &Config) -> Vec<String> {
    let assistant = &config.acp.agent;
    let mut parts: Vec<String> = Vec::new();

    // Built-in steering (always first)
    parts.push(BUILTIN_STEERING.to_string());

    // Current date and time — so the LLM knows the actual date for relative queries
    let now = chrono::Local::now();
    parts.push(format!(
        "<current_datetime>\nCurrent date and time: {}\nTimezone: {}\n</current_datetime>",
        now.format("%A, %B %e, %Y %I:%M %p"),
        now.format("%Z"),
    ));

    // User-written steering doc
    if let Some(ref path) = assistant.user_steering_path {
        if !path.is_empty() {
            match fs::read_to_string(path) {
                Ok(content) if !content.trim().is_empty() => {
                    info!("Loaded user steering doc from: {}", path);
                    parts.push(content);
                }
                Ok(_) => {}
                Err(e) => error!("Failed to read user steering doc {}: {}", path, e),
            }
        }
    }

    // Auto-generated steering doc
    if assistant.auto_steering_enabled {
        match Config::get_auto_steering_path() {
            Ok(auto_path) => {
                if auto_path.exists() {
                    match fs::read_to_string(&auto_path) {
                        Ok(content) if !content.trim().is_empty() => {
                            parts.push(content);
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => error!("Failed to get auto steering path: {}", e),
        }
    }

    parts
}

/// Format assembled steering parts into a complete steering message
/// with the prefix and ack instruction.
pub fn format_steering_message(parts: &[String]) -> String {
    format!(
        "{} {}\n\n---\n\n<instructions>Respond with only \"ack\" to confirm receipt. Do not summarize or comment on the content above.</instructions>",
        STEERING_MSG_PREFIX,
        parts.join("\n\n---\n\n")
    )
}

#[tauri::command]
pub async fn get_config(features: State<'_, FeatureServices>) -> Result<Config, AppError> {
    let config = features.config.lock_or_recover();
    Ok(config.clone())
}

#[tauri::command]
pub async fn save_config(
    config: Config,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    info!("Saving configuration");

    // Swap the in-memory state and persist under the same lock. Pre-fix the
    // sequence was disk-write → lock → swap, which raced with concurrent
    // saves (e.g. handle_permission_notification updating last_seen): the
    // disk write could capture a stale view, get clobbered by an in-flight
    // permission save, then the in-memory swap would proceed against an
    // even staler picture. Lock-mutate-save-drop is the only correct order.
    let new_terminator = config.tool_permissions.terminator_mode;
    let new_log_buffer_size = config.system.log_buffer_size;
    let prior_terminator = {
        let mut state_config = features.config.lock_or_recover();
        let prior = state_config.tool_permissions.terminator_mode;
        *state_config = config;
        // Normalize the update channel — unknown values collapse to
        // "stable". This guards against a frontend bug, a stale config
        // migration, or an end-user hand-editing config.json to a
        // channel we don't understand.
        state_config.updates.channel =
            crate::updater::normalize_channel(&state_config.updates.channel).to_string();
        state_config.save().map_err(|e| {
            error!("Failed to save config: {}", e);
            format!("Failed to save configuration: {}", e)
        })?;
        prior
    };

    // Update app log buffer size if changed
    crate::app_log::set_max_size(new_log_buffer_size);

    if prior_terminator != new_terminator {
        crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
            crate::permission_audit::AuditEvent::TerminatorModeChanged {
                enabled: new_terminator,
            },
        ));
    }

    info!("Configuration saved successfully");

    if let Err(e) = app.emit("config_updated", ()) {
        error!("Failed to emit config_updated event: {}", e);
    }

    Ok(())
}

#[tauri::command]
pub async fn save_frecency(data: String) -> Result<(), AppError> {
    let path = dirs::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("search-frecency.json");
    Ok(std::fs::write(&path, &data).map_err(|e| format!("Failed to save frecency: {}", e))?)
}

#[tauri::command]
pub async fn load_frecency() -> Result<String, AppError> {
    let path = dirs::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("search-frecency.json");
    match std::fs::read_to_string(&path) {
        Ok(data) => Ok(data),
        Err(_) => Ok("{}".to_string()),
    }
}

const MAX_SHORTCUT_HISTORY: usize = 20;

fn shortcut_history_path() -> Result<std::path::PathBuf, String> {
    Ok(dirs::config_dir()
        .ok_or("No config dir")?
        .join("kage")
        .join("shortcut-history.json"))
}

/// Record a shortcut execution with arguments for history recall.
#[tauri::command]
pub async fn record_shortcut_usage(trigger: String, args: String) -> Result<(), AppError> {
    let args = args.trim().to_string();
    if args.is_empty() {
        return Ok(());
    }

    let path = shortcut_history_path()?;
    let mut history: serde_json::Map<String, serde_json::Value> = if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    let entry = serde_json::json!({
        "args": args,
        "at": chrono::Utc::now().to_rfc3339()
    });

    let entries = history
        .entry(trigger)
        .or_insert_with(|| serde_json::json!([]));
    if let Some(arr) = entries.as_array_mut() {
        // Remove duplicate if same args already exist
        arr.retain(|e| {
            e.get("args").and_then(|a| a.as_str()) != Some(entry["args"].as_str().unwrap_or(""))
        });
        // Prepend new entry
        arr.insert(0, entry);
        // Cap at MAX_SHORTCUT_HISTORY
        arr.truncate(MAX_SHORTCUT_HISTORY);
    }

    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    Ok(fs::write(
        &path,
        serde_json::to_string_pretty(&history).unwrap_or_default(),
    )
    .map_err(|e| format!("Failed to save shortcut history: {}", e))?)
}

/// Get history entries for a specific shortcut trigger.
#[tauri::command]
pub async fn get_shortcut_history(trigger: String) -> Result<Vec<serde_json::Value>, AppError> {
    let path = shortcut_history_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }

    let history: serde_json::Map<String, serde_json::Value> = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    Ok(history
        .get(&trigger)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

#[tauri::command]
pub async fn update_tool_policy(
    tool_title: String,
    policy: String,
    grant_type: Option<String>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let gt = grant_type.unwrap_or_else(|| "once".to_string());
    info!(
        "Updating tool policy: {} -> {} (grant: {})",
        tool_title, policy, gt
    );
    let mut config = features.config.lock_or_recover();
    let timestamp = chrono::Utc::now().to_rfc3339();

    // Capture the prior state so the audit log can say "you changed
    // from allow/always to deny" etc. We emit BEFORE the struct is
    // mutated so a crash mid-write doesn't leave us logging the
    // wrong transition.
    let prior = config
        .tool_permissions
        .tools
        .iter()
        .find(|t| t.title == tool_title)
        .map(|t| (t.policy.clone(), t.grant_type.clone()));

    if let Some(tool) = config
        .tool_permissions
        .tools
        .iter_mut()
        .find(|t| t.title == tool_title)
    {
        tool.policy = policy.clone();
        tool.grant_type = gt.clone();
        tool.granted_at = timestamp;
    }
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    drop(config);

    // Log the transition. "allow" is a grant; "deny" or "ask" is a
    // revoke-style change when the prior policy was "allow".
    let event = match policy.as_str() {
        "allow" => crate::permission_audit::AuditEvent::Granted {
            tool: tool_title,
            grant_type: gt,
            session_id: None,
            args_preview: None,
        },
        _ => {
            if let Some((prior_policy, prior_gt)) = prior.filter(|(p, _)| p == "allow") {
                crate::permission_audit::AuditEvent::Revoked {
                    tool: tool_title,
                    prior_policy,
                    prior_grant_type: Some(prior_gt),
                }
            } else {
                // Transitioning from ask→deny or ask→ask; not interesting.
                return Ok(());
            }
        }
    };
    crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(event));
    Ok(())
}

#[tauri::command]
pub async fn remove_tool_permission(
    tool_title: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();

    // Snapshot the policy we're about to drop so we can log what was
    // revoked, not just that something was.
    let prior = config
        .tool_permissions
        .tools
        .iter()
        .find(|t| t.title == tool_title)
        .map(|t| (t.policy.clone(), t.grant_type.clone()));

    config
        .tool_permissions
        .tools
        .retain(|t| t.title != tool_title);
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    drop(config);

    if let Some((prior_policy, prior_gt)) = prior {
        crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
            crate::permission_audit::AuditEvent::Revoked {
                tool: tool_title,
                prior_policy,
                prior_grant_type: Some(prior_gt),
            },
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn is_dev_mode(ui: State<'_, UiState>) -> Result<bool, AppError> {
    Ok(ui.dev_mode)
}

#[tauri::command]
pub async fn is_terminator_mode(features: State<'_, FeatureServices>) -> Result<bool, AppError> {
    let config = features.config.lock_or_recover();
    Ok(config.tool_permissions.terminator_mode)
}

#[tauri::command]
pub async fn open_welcome_window(app: tauri::AppHandle) -> Result<(), AppError> {
    use tauri::WebviewWindowBuilder;
    // If window exists and is valid, just focus it
    if let Some(w) = app.get_webview_window("welcome") {
        let _ = w.show();
        let _ = w.set_focus();
        crate::setup::update_activation_policy(&app);
        return Ok(());
    }
    // Create fresh window (previous one was closed/destroyed)
    let w = WebviewWindowBuilder::new(
        &app,
        "welcome",
        tauri::WebviewUrl::App("welcome.html".into()),
    )
    .title("Welcome to Kage")
    .inner_size(580.0, 640.0)
    .resizable(false)
    .decorations(false)
    .center()
    .visible(false) // Hidden until content loads
    .build()
    .map_err(|e| format!("Failed to open welcome window: {}", e))?;
    // Set dark background to prevent white flash
    let _ = w.set_background_color(Some(tauri::window::Color(30, 26, 36, 255)));
    // When closed, destroy so it can be recreated
    let w2 = w.clone();
    let app2 = app.clone();
    w.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { .. } = event {
            let _ = w2.destroy();
            crate::setup::update_activation_policy(&app2);
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn complete_first_run(
    app: tauri::AppHandle,
    features: State<'_, FeatureServices>,
    launch_at_startup: bool,
    auto_update: bool,
    enable_computer_control: bool,
    enable_personalization: bool,
    enable_telemetry: bool,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.first_run_completed = true;
    if auto_update {
        config.updates.auto_check = true;
        config.updates.silent_update = true;
    }
    config.acp.agent.auto_steering_enabled = enable_personalization;
    let _ = config.save();
    drop(config);

    // Apply the telemetry decision. `set_consent` generates a fresh
    // install_id on first opt-in and records the privacy policy version
    // the user saw — the UI re-prompts if the version ever changes.
    crate::telemetry::set_consent(&features.config, enable_telemetry);

    set_startup_enabled_impl(launch_at_startup);

    // Register the computer-control MCP server if the user opted in
    if enable_computer_control {
        crate::mcp_registration::ensure_registered();
    }

    // Track the first-run outcome. This runs *after* set_consent so it
    // respects the user's choice — if they opted out, no event is sent.
    crate::telemetry::track(
        &app,
        "first_run_completed",
        Some(serde_json::json!({
            "telemetry": if enable_telemetry { "opted_in" } else { "opted_out" },
            "startup": if launch_at_startup { "on" } else { "off" },
            "auto_update": if auto_update { "on" } else { "off" },
            "computer_control": if enable_computer_control { "on" } else { "off" },
            "personalization": if enable_personalization { "on" } else { "off" },
        })),
    );

    // NOTE: the welcome banner / floating window display used to be
    // triggered from here. That's moved to the `trigger_welcome_banner`
    // command so the welcome UI can pick the precise moment — part of
    // a fade-in choreography where the floating window appears midway
    // through the welcome window's fade-out.

    Ok(())
}

/// Show the floating window and welcome banner. Called from the welcome
/// UI's Finish-flow choreography (not automatically from
/// `complete_first_run` — we want precise control over when the
/// floating UI appears relative to the welcome window's fade-out).
#[tauri::command]
pub async fn trigger_welcome_banner(app: tauri::AppHandle) -> Result<(), AppError> {
    show_welcome_banner(&app);
    Ok(())
}

/// Show the floating window with a welcome banner displaying the configured hotkey.
/// Called from first-run completion and the dev tray menu.
pub fn show_welcome_banner(app: &tauri::AppHandle) {
    let hotkey_str = app
        .try_state::<FeatureServices>()
        .and_then(|features| {
            features
                .config
                .try_lock()
                .ok()
                .map(|c| c.get_hotkey_string())
        })
        .unwrap_or_else(|| "Alt+Space".to_string());
    let keycaps: String = hotkey_str
        .split('+')
        .map(|k| format!("<span class=\"keycap\">{}</span>", k))
        .collect::<Vec<_>>()
        .join("<span class=\"keycap-sep\">+</span>");
    let text = format!(
        "<b>Welcome to Kage!</b><br/>&nbsp;<br>Press {} anytime to summon me.",
        keycaps
    );

    if let Some(floating) = app.get_webview_window("floating") {
        crate::commands::window::center_floating_on_active_monitor(&floating);
        let _ = floating.show();
        let _ = floating.set_focus();
    }
    let _ = app.emit(
        "show_floating_banner",
        serde_json::json!({
            "icon": "👋",
            "text": text,
            "action_label": "",
            "action_type": "dismiss",
            "action_data": ""
        }),
    );
}

#[tauri::command]
pub async fn is_first_run(features: State<'_, FeatureServices>) -> Result<bool, AppError> {
    let config = features.config.lock_or_recover();
    Ok(!config.first_run_completed)
}

#[tauri::command]
pub async fn get_startup_enabled() -> Result<bool, AppError> {
    Ok(get_startup_enabled_impl())
}

#[tauri::command]
pub async fn set_startup_enabled(enabled: bool) -> Result<(), AppError> {
    set_startup_enabled_impl(enabled);
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

fn get_startup_enabled_impl() -> bool {
    crate::os::get_startup_enabled()
}

fn set_startup_enabled_impl(enabled: bool) {
    crate::os::set_startup_enabled(enabled);
}

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
        // rather than maintaining a parallel hardcoded list. See
        // updater::VALID_CHANNELS.
        "update_channels": crate::updater::VALID_CHANNELS,
    }))
}

/// Detect whether the OS is using dark mode.
#[tauri::command]
pub async fn get_os_dark_mode() -> bool {
    crate::os::is_dark_mode()
}

/// Register all global hotkeys from config. Unregisters everything first.
/// This is the single source of truth for hotkey registration — called from:
/// - App startup (main.rs)
/// - Config changes (config_updated listener)
/// - After hotkey capture (capture_hotkey_combo)
/// - After hotkey test (try_register_hotkey)
pub fn register_all_hotkeys(app: &tauri::AppHandle) {
    use tauri::Emitter;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    info!("Registering all hotkeys...");
    let _ = app.global_shortcut().unregister_all();

    let features: tauri::State<'_, FeatureServices> = app.state();
    let config = features.config.lock_or_recover();
    let main_hk = config.get_hotkey_string();
    let cb_hk = config.get_clipboard_hotkey_string();
    let ia_hk = config.get_inline_assist_hotkey_string();
    let voice_hk = config.get_voice_hotkey_string();
    drop(config);

    // --- Main hotkey: toggle floating window (unique behavior) ---
    if let Some(floating) = app.get_webview_window("floating") {
        match app
            .global_shortcut()
            .on_shortcut(main_hk.as_str(), move |_app, _shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }
                info!("Hotkey triggered: main ({})", _shortcut);
                crate::commands::window::toggle_floating_window(&floating);
            }) {
            Ok(_) => info!("✅ Registered main hotkey: {}", main_hk),
            Err(e) => error!("❌ Failed to register main hotkey {}: {}", main_hk, e),
        }
    }

    // --- Inline assist hotkey: capture selection + show assist (unique behavior) ---
    if let Some(ref ia) = ia_hk {
        let ia_handle = app.clone();
        match app
            .global_shortcut()
            .on_shortcut(ia.as_str(), move |_app, _shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }
                info!("Hotkey triggered: inline-assist ({})", _shortcut);
                let source_info = crate::os::window_list::get_foreground_window_info();
                let features: tauri::State<'_, FeatureServices> = ia_handle.state();
                let blocklist = features
                    .config
                    .lock_or_recover()
                    .system
                    .capture_selection_blocklist
                    .clone();
                let fg_process = source_info
                    .as_ref()
                    .map(|(_, proc)| proc.as_str())
                    .unwrap_or("");
                let selection =
                    if crate::os::clipboard::is_process_blocklisted(fg_process, &blocklist) {
                        info!(
                            "[inline-assist] Skipping capture — foreground app '{}' is blocklisted",
                            fg_process
                        );
                        None
                    } else {
                        let capture_token = crate::os::clipboard::begin_selection_capture();
                        crate::os::clipboard::finish_selection_capture(capture_token)
                    };
                let cursor_pos = crate::os::cursor::get_cursor_position().unwrap_or((500, 500));
                let handle = ia_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::commands::window::show_inline_assist_with_context(
                        handle,
                        source_info,
                        selection,
                        cursor_pos,
                    )
                    .await
                    {
                        error!("Failed to show inline assist: {}", e);
                    }
                });
            }) {
            Ok(_) => info!("✅ Registered inline-assist hotkey: {}", ia),
            Err(e) => warn!("❌ Failed to register inline-assist hotkey {}: {}", ia, e),
        }
    } else {
        info!("ℹ️ No inline-assist hotkey configured");
    }

    // --- Event-based hotkeys: show floating at mouse, then emit a frontend event ---
    // To add a new hotkey of this type: add a config getter and an entry here.
    let event_hotkeys: Vec<(&str, Option<String>, &str, u64)> = vec![
        // (name,        hotkey_string, event_name,              delay_ms)
        ("clipboard", cb_hk, "clipboard_history_mode", 150),
        ("voice", voice_hk, "voice_mode", 200),
    ];

    for (name, hk_opt, event_name, delay_ms) in event_hotkeys {
        match hk_opt {
            Some(ref hk) => {
                if let Some(floating) = app.get_webview_window("floating") {
                    let app_handle = app.clone();
                    let evt = event_name.to_string();
                    let label = name.to_string();
                    match app.global_shortcut().on_shortcut(
                        hk.as_str(),
                        move |_app, _shortcut, event| {
                            if event.state != ShortcutState::Pressed {
                                return;
                            }
                            info!("Hotkey triggered: {} ({})", label, _shortcut);
                            crate::commands::window::show_floating_at_mouse(&floating);
                            let handle = app_handle.clone();
                            let evt = evt.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                                let _ = handle.emit(&evt, ());
                            });
                        },
                    ) {
                        Ok(_) => info!("✅ Registered {} hotkey: {}", name, hk),
                        Err(e) => warn!("❌ Failed to register {} hotkey {}: {}", name, hk, e),
                    }
                }
            }
            None => info!("ℹ️ No {} hotkey configured", name),
        }
    }
}

#[tauri::command]
pub async fn try_register_hotkey(
    app: tauri::AppHandle,
    modifiers: Vec<String>,
    key: String,
    slot: Option<String>,
) -> Result<bool, AppError> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let hotkey_str = if modifiers.is_empty() {
        key.clone()
    } else {
        format!("{}+{}", modifiers.join("+"), key)
    };
    info!(
        "Trying to register hotkey: {} (slot: {:?})",
        hotkey_str, slot
    );

    // Check for conflicts with other hotkey slots
    {
        let features: tauri::State<'_, FeatureServices> = app.state();
        let config = features.config.lock_or_recover();
        let main_hk = config.get_hotkey_string();
        let cb_hk = config.get_clipboard_hotkey_string();
        let ia_hk = config.get_inline_assist_hotkey_string();
        let slot_name = slot.as_deref().unwrap_or("main");

        let normalize = |s: &str| -> String {
            let mut parts: Vec<String> = s.split('+').map(|p| p.trim().to_lowercase()).collect();
            if parts.len() > 1 {
                let key = parts.pop().unwrap();
                parts.sort();
                parts.push(key);
            }
            parts.join("+")
        };
        let new_norm = normalize(&hotkey_str);

        // Check all other slots for conflicts
        let all_hotkeys: Vec<(&str, String)> = [
            ("main", Some(main_hk)),
            ("clipboard", cb_hk),
            ("inline-assist", ia_hk),
        ]
        .into_iter()
        .filter(|(name, _)| *name != slot_name)
        .filter_map(|(name, hk)| hk.map(|h| (name, normalize(&h))))
        .collect();

        for (name, norm) in &all_hotkeys {
            if new_norm == *norm {
                return Err(format!("This shortcut is already used as the {} hotkey", name).into());
            }
        }

        // If it's the same as the current value for this slot, no change needed
        let current_for_slot = match slot_name {
            "main" => Some(normalize(&config.get_hotkey_string())),
            "clipboard" => config.get_clipboard_hotkey_string().map(|s| normalize(&s)),
            "inline-assist" => config
                .get_inline_assist_hotkey_string()
                .map(|s| normalize(&s)),
            _ => None,
        };
        if current_for_slot.as_deref() == Some(new_norm.as_str()) {
            return Ok(true);
        }
    }

    // Test that the hotkey can be registered
    let _ = app.global_shortcut().unregister_all();
    match app
        .global_shortcut()
        .on_shortcut(hotkey_str.as_str(), |_app, _shortcut, _event| {})
    {
        Ok(_) => {
            info!("✅ Hotkey test passed: {}", hotkey_str);
            // Re-register all hotkeys (the config hasn't been saved yet,
            // but the frontend will save it and trigger config_updated)
            register_all_hotkeys(&app);
            Ok(true)
        }
        Err(e) => {
            let msg = format!("{}", e);
            info!("❌ Hotkey registration failed: {}", msg);
            // Restore all hotkeys from config
            register_all_hotkeys(&app);
            Err(msg.into())
        }
    }
}

#[tauri::command]
pub async fn capture_hotkey_combo(app: tauri::AppHandle) -> Result<serde_json::Value, AppError> {
    // Temporarily unregister global hotkeys so they don't intercept during capture
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister_all();

    let result = tauri::async_runtime::spawn_blocking(|| crate::os::capture_hotkey(10000))
        .await
        .map_err(|e| format!("Task error: {}", e))?;

    // Re-register all global hotkeys from config
    register_all_hotkeys(&app);

    match result {
        Some(captured) => Ok(serde_json::json!({
            "modifiers": captured.modifiers,
            "key": captured.key,
            "display": captured.display,
        })),
        None => Ok(serde_json::json!(null)),
    }
}

#[tauri::command]
pub async fn cancel_hotkey_capture() -> Result<(), AppError> {
    crate::os::cancel_hotkey_capture();
    Ok(())
}

#[tauri::command]
pub async fn open_devtools(app: tauri::AppHandle) -> Result<(), AppError> {
    #[cfg(debug_assertions)]
    if let Some(window) = app.get_webview_window("floating") {
        let window: tauri::WebviewWindow = window;
        window.open_devtools();
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
    }
    Ok(())
}

#[tauri::command]
pub async fn restart_app(app: tauri::AppHandle) -> Result<(), AppError> {
    info!("Restart requested via > command");

    let exe = std::env::current_exe().map_err(|e| format!("Failed to get exe path: {}", e))?;
    // Filter args: only keep our app flags, not cargo flags (--no-default-features, --color, etc.)
    let mut args: Vec<String> = Vec::new();
    let mut skip_next = false;
    for arg in std::env::args().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--" {
            break;
        } // Stop at cargo separator
        if arg.starts_with("--no-default") || arg.starts_with("--color") {
            if arg == "--color" {
                skip_next = true;
            } // --color has a value arg
            continue;
        }
        args.push(arg);
    }
    if !args.iter().any(|a| a == "--restart" || a == "/restart") {
        args.push(
            if cfg!(windows) {
                "/restart"
            } else {
                "--restart"
            }
            .to_string(),
        );
    }

    graceful_shutdown(&app);

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        shutdown_and_exit_with_restart(&app_handle, exe, args).await;
    });

    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: tauri::AppHandle) -> Result<(), AppError> {
    info!("Quit requested via > command");
    graceful_shutdown(&app);

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        shutdown_and_exit(&app_handle).await;
    });

    Ok(())
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
    Ok(
        tauri::async_runtime::spawn_blocking(move || crate::os::search_files(&q, max))
            .await
            .map_err(|e| format!("Search task failed: {}", e))?,
    )
}

/// Get upcoming calendar events.
#[tauri::command]
pub async fn get_calendar_events(
    hours: Option<u32>,
) -> Result<Vec<crate::os::calendar::CalendarEvent>, AppError> {
    let h = hours.unwrap_or(24).min(72);
    tauri::async_runtime::spawn_blocking(move || crate::os::get_upcoming_events(h))
        .await
        .map_err(|e| AppError::from(format!("Calendar task failed: {}", e)))?
        .map_err(AppError::from)
}

/// Get calendar events for a specific date (YYYY-MM-DD).
#[tauri::command]
pub async fn get_calendar_events_for_date(
    date: String,
) -> Result<Vec<crate::os::calendar::CalendarEvent>, AppError> {
    // Strict YYYY-MM-DD validation. The date is interpolated into a PowerShell
    // command on Windows, so anything more permissive is an injection vector.
    if date.len() != 10 {
        return Err("Invalid date format. Use YYYY-MM-DD.".into());
    }
    let bytes = date.as_bytes();
    let ok = bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[5].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7] == b'-'
        && bytes[8].is_ascii_digit()
        && bytes[9].is_ascii_digit();
    if !ok {
        return Err("Invalid date format. Use YYYY-MM-DD.".into());
    }
    tauri::async_runtime::spawn_blocking(move || crate::os::get_events_for_date(&date))
        .await
        .map_err(|e| AppError::from(format!("Calendar date query failed: {}", e)))?
        .map_err(AppError::from)
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
    // Detect content type from magic bytes
    let content_type = if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        "image/x-icon"
    } else {
        "image/png" // default
    };

    Ok(format!("data:{};base64,{}", content_type, b64))
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
fn message_length_bucket(n: usize) -> &'static str {
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

/// Build the combined steering content from user and auto-generated docs.
/// User steering takes precedence (placed first).
/// Returns None if no steering content is available.
#[tauri::command]
pub async fn get_steering_content(
    features: State<'_, FeatureServices>,
) -> Result<Option<String>, AppError> {
    let config = features.config.lock_or_recover();
    let parts = assemble_steering_parts(&config);
    Ok(Some(format_steering_message(&parts)))
}

/// Open the auto-generated steering document in the default editor.
/// Creates the file with a header comment if it doesn't exist yet.
#[tauri::command]
pub async fn open_auto_steering_file() -> Result<String, AppError> {
    let auto_path = Config::get_auto_steering_path()
        .map_err(|e| format!("Failed to get auto steering path: {}", e))?;

    // Create with header if it doesn't exist
    if !auto_path.exists() {
        if let Some(parent) = auto_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
        }
        let header = "<!-- AUTO-GENERATED STEERING DOCUMENT\n     Any manual changes will be overridden the next time this document is generated.\n     To add your own persistent instructions, use a User Steering Document instead. -->\n\n";
        fs::write(&auto_path, header)
            .map_err(|e| format!("Failed to create auto steering file: {}", e))?;
    }

    let path_str = auto_path
        .to_str()
        .ok_or_else(|| "Invalid path encoding".to_string())?
        .to_string();

    // Open in default editor
    crate::os::open_in_editor(&path_str).map_err(|e| format!("Failed to open file: {}", e))?;

    Ok(path_str)
}

/// Get the path to the auto-generated steering document
#[tauri::command]
pub async fn get_auto_steering_path() -> Result<String, AppError> {
    Ok(Config::get_auto_steering_path()
        .map_err(|e| format!("Failed to get path: {}", e))
        .and_then(|p| {
            p.to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "Invalid path encoding".to_string())
        })?)
}

// --- Line-editor IO for the steering documents ----------------------
//
// The Personalization settings page edits the auto and user steering
// docs as line lists. Read returns the resolved path so the UI can
// render "we'll save this to …" + a Reveal-in-Explorer affordance.
// Write updates `user_steering_path` in config the first time a user
// saves a non-empty doc with no path configured — that pins the
// default location into the user's settings so future installs find
// it.

fn parse_steering_kind(kind: &str) -> Result<crate::steering_io::SteeringKind, AppError> {
    crate::steering_io::SteeringKind::parse(kind)
        .ok_or_else(|| AppError::from(format!("Unknown steering kind: {}", kind)))
}

fn path_to_string(p: &std::path::Path) -> Result<String, AppError> {
    p.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::from("Invalid path encoding".to_string()))
}

#[tauri::command]
pub async fn read_steering_lines(
    kind: String,
    features: State<'_, FeatureServices>,
) -> Result<serde_json::Value, AppError> {
    let kind = parse_steering_kind(&kind)?;
    let config = features.config.lock_or_recover();
    let (path, lines) = crate::steering_io::read_lines(kind, &config)
        .map_err(|e| format!("Failed to read steering doc: {}", e))?;
    Ok(serde_json::json!({
        "path": path_to_string(&path)?,
        "lines": lines,
        "exists": path.exists(),
    }))
}

#[tauri::command]
pub async fn write_steering_lines(
    kind: String,
    lines: Vec<String>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, AppError> {
    let kind = parse_steering_kind(&kind)?;

    // Capture whether the user had no explicit user_steering_path
    // configured before this write — that's the trigger for pinning
    // the default location into config so subsequent reads agree.
    let needs_path_persist = matches!(kind, crate::steering_io::SteeringKind::User) && {
        let cfg = features.config.lock_or_recover();
        cfg.acp
            .agent
            .user_steering_path
            .as_deref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
    };

    let path = {
        let config = features.config.lock_or_recover();
        crate::steering_io::write_lines(kind, &config, &lines)
            .map_err(|e| format!("Failed to write steering doc: {}", e))?
    };

    if needs_path_persist {
        let mut config = features.config.lock_or_recover();
        config.acp.agent.user_steering_path = Some(path_to_string(&path)?);
        if let Err(e) = config.save() {
            warn!("Failed to persist user_steering_path: {}", e);
        }
        // Drop the lock before emitting so listeners that re-read
        // config don't deadlock.
        drop(config);
        let _ = app.emit("config_updated", ());
    }

    Ok(serde_json::json!({
        "path": path_to_string(&path)?,
    }))
}

/// Read an arbitrary file path the user picked via the file dialog.
/// Returns its lines without touching the on-disk steering doc — the
/// frontend decides whether to merge with existing content or
/// replace, and writes via `write_steering_lines`.
#[tauri::command]
pub async fn import_steering_lines(path: String) -> Result<Vec<String>, AppError> {
    crate::steering_io::import_lines_from_path(&path)
        .map_err(|e| AppError::from(format!("Failed to import steering doc: {}", e)))
}

// --- Cross-device backup commands -----------------------------------
//
// Export bundles the user's current config + steering docs +
// extension data into a single zip (optionally AES-GCM-encrypted with
// a passphrase). Import is the inverse — unwraps, sanitises away
// device-local fields, and writes everything back. See
// `src/config_export.rs` for the layout + sanitisation rules.

/// Suggested filename for the export dialog. Frontend uses this so
/// the date stamp matches Local time on the user's machine without
/// the JS having to format dates itself.
#[tauri::command]
pub async fn export_config_default_filename(encrypted: bool) -> String {
    crate::config_export::default_filename(encrypted)
}

/// Build the backup bytes and write them to the user-picked path.
/// Returns the byte count so the UI can confirm a successful write.
/// We do the disk write here rather than handing bytes back over IPC
/// because a config + extension-data bundle can be tens of MB and
/// the round-trip is wasteful.
#[tauri::command]
pub async fn export_config_bundle(
    path: String,
    passphrase: Option<String>,
    features: State<'_, FeatureServices>,
) -> Result<u64, AppError> {
    let config = {
        let cfg = features.config.lock_or_recover();
        cfg.clone()
    };
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        crate::config_export::export(&config, passphrase.as_deref())
    })
    .await
    .map_err(|e| AppError::from(format!("Export task failed: {}", e)))?
    .map_err(|e| AppError::from(format!("Failed to build backup: {}", e)))?;

    let len = bytes.len() as u64;
    std::fs::write(&path, &bytes)
        .map_err(|e| AppError::from(format!("Failed to write backup to {}: {}", path, e)))?;
    Ok(len)
}

/// Return the most recent unseen crash summary (or null) so the
/// floating window can decide whether to surface a "Kage crashed
/// last session" banner. Returns null when there's no crash log,
/// when the latest report has already been acknowledged, or when
/// the file is too malformed to parse useful fields out of.
///
/// Acknowledging happens via `dismiss_recent_crash` — that command
/// stamps `system.last_seen_crash_timestamp` so subsequent
/// invocations of THIS command return null until a NEW crash is
/// recorded.
#[tauri::command]
pub async fn get_recent_crash(
    features: State<'_, FeatureServices>,
) -> Result<Option<crate::crash_recovery::CrashSummary>, AppError> {
    let summary = match crate::crash_recovery::read_recent_crash() {
        Some(s) => s,
        None => return Ok(None),
    };
    let last_seen = features
        .config
        .lock_or_recover()
        .system
        .last_seen_crash_timestamp
        .clone();
    if !crate::crash_recovery::is_unseen(&summary, last_seen.as_deref()) {
        return Ok(None);
    }
    Ok(Some(summary))
}

/// Mark a crash report as acknowledged. The frontend calls this when
/// the user clicks any of the banner actions (View log / Report /
/// Dismiss) so the dialog doesn't fire again on the next launch.
/// `timestamp` must match the value returned by `get_recent_crash`
/// — we don't trust an arbitrary string from the UI to overwrite the
/// stamp, but the report header is the canonical id and the UI
/// already has it.
#[tauri::command]
pub async fn dismiss_recent_crash(
    timestamp: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let mut cfg = features.config.lock_or_recover();
    cfg.system.last_seen_crash_timestamp = Some(timestamp);
    let _ = cfg.save();
    Ok(())
}

/// Write a UTF-8 string to the given path. The frontend uses this for
/// any "save text" workflow that's already routed a path through the
/// Tauri dialog plugin (chat markdown export, today; future "save as"
/// flows can land here too without the frontend needing a new
/// command). Returns the byte count so the caller can confirm a
/// successful write.
///
/// Why an in-process command rather than `tauri-plugin-fs`: the fs
/// plugin requires per-path scope grants in `capabilities/`, which
/// would force us to pre-declare every directory the user might
/// pick. The dialog plugin already returns an absolute path the user
/// just consented to, so we just write it.
#[tauri::command]
pub async fn write_text_file(path: String, contents: String) -> Result<u64, AppError> {
    let len = contents.len() as u64;
    std::fs::write(&path, contents.as_bytes())
        .map_err(|e| AppError::from(format!("Failed to write {}: {}", path, e)))?;
    Ok(len)
}

/// Apply a backup bundle from disk. Returns the import summary (counts)
/// so the UI can render a concrete success toast. The new config is
/// written to disk + the in-memory state is replaced; a `config_updated`
/// Tauri event lets every window reload theme / shortcuts / etc.
#[tauri::command]
pub async fn import_config_bundle(
    path: String,
    passphrase: Option<String>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<crate::config_export::ImportSummary, AppError> {
    let bytes = std::fs::read(&path)
        .map_err(|e| AppError::from(format!("Failed to read backup at {}: {}", path, e)))?;
    let local = {
        let cfg = features.config.lock_or_recover();
        cfg.clone()
    };

    // Disk + decryption work happens off the runtime so the dialog
    // stays responsive even with Argon2's intentionally-slow KDF.
    let (new_config, summary) = tauri::async_runtime::spawn_blocking(move || {
        crate::config_export::import(&bytes, passphrase.as_deref(), &local)
    })
    .await
    .map_err(|e| AppError::from(format!("Import task failed: {}", e)))?
    .map_err(|e| AppError::from(format!("Failed to import backup: {}", e)))?;

    // Persist + swap the in-memory state under the same lock —
    // mirrors save_config so concurrent permission saves don't race.
    let new_log_buffer = new_config.system.log_buffer_size;
    let new_terminator = new_config.tool_permissions.terminator_mode;
    let prior_terminator = {
        let mut state_config = features.config.lock_or_recover();
        let prior = state_config.tool_permissions.terminator_mode;
        *state_config = new_config;
        // Same channel-string normalisation as save_config so a hand-
        // edited backup can't trap a user on a dead channel.
        state_config.updates.channel =
            crate::updater::normalize_channel(&state_config.updates.channel).to_string();
        state_config
            .save()
            .map_err(|e| format!("Failed to persist imported config: {}", e))?;
        prior
    };

    crate::app_log::set_max_size(new_log_buffer);
    if prior_terminator != new_terminator {
        crate::permission_audit::append(&crate::permission_audit::AuditEntry::now(
            crate::permission_audit::AuditEvent::TerminatorModeChanged {
                enabled: new_terminator,
            },
        ));
    }

    // Re-apply the OS-level startup hook so it agrees with whatever
    // value is now in config (we already sanitised it to keep the
    // local preference, so this is essentially a no-op write — we
    // run it anyway so the registry / launchd entry state can never
    // drift from the JSON).
    let auto_start = {
        let cfg = features.config.lock_or_recover();
        cfg.system.auto_start
    };
    set_startup_enabled_impl(auto_start);

    let _ = app.emit("config_updated", ());
    Ok(summary)
}

// --- App Modes / context rules --------------------------------------
//
// The floating window's send path calls `match_context_rule` with the
// foreground process name (already captured for the `<_kage_ctx>`
// tag). On a hit we return both the formatted `<_kage_app_steering>`
// payload (ready to splice into the prompt) and the rule's friendly
// name so the chip in the input bar can show "🎯 VS Code mode" before
// the user sends. Returning `None` is fine — the caller skips the
// injection entirely. See `src/context_rules.rs` for the matcher.

#[derive(Debug, Clone, serde::Serialize)]
pub struct MatchedContextRule {
    pub friendly_name: String,
    pub steering_payload: String,
}

#[tauri::command]
pub async fn match_context_rule(
    executable: String,
    features: State<'_, FeatureServices>,
) -> Result<Option<MatchedContextRule>, AppError> {
    let cfg = features.config.lock_or_recover();
    let Some(rule) = crate::context_rules::first_matching(&cfg.context_rules, &executable) else {
        return Ok(None);
    };
    let Some(payload) = crate::context_rules::format_steering_payload(rule) else {
        // Rule matched but its steering body is empty — treat as a
        // no-op so the chip doesn't claim a mode that contributes
        // nothing to the prompt.
        return Ok(None);
    };
    Ok(Some(MatchedContextRule {
        friendly_name: rule.friendly_name.clone(),
        steering_payload: payload,
    }))
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
        .map_err(|e| AppError::from(format!("Failed to list Ollama models: {}", e)))
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

// --- Update commands ---

#[tauri::command]
pub async fn check_for_update(
    app: tauri::AppHandle,
    features: State<'_, FeatureServices>,
) -> Result<serde_json::Value, AppError> {
    let channel = {
        let cfg = features.config.lock_or_recover();
        cfg.updates.channel.clone()
    };

    let result = crate::updater::plugin_check(&app, &channel)
        .await
        .map_err(|e| format!("Check failed: {}", e))?;

    let available = result.as_ref().map(|u| u.version.clone());

    // Cache the Update handle so download_and_install_update can
    // consume it without re-checking.
    if let Some(update) = result {
        if let Ok(mut v) = features.updater.available_version.lock() {
            *v = Some(update.version.clone());
        }
        if let Ok(mut p) = features.updater.pending_update.lock() {
            *p = Some(update);
        }
        features
            .updater
            .update_ready
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    // Emit event so the floating window can show a banner too.
    if let Some(ref version) = available {
        let _ = app.emit("update_available", version);
    }

    Ok(serde_json::json!({
        "current_version": crate::updater::CURRENT_VERSION,
        "available_version": available,
    }))
}

#[tauri::command]
pub async fn fetch_changelog(features: State<'_, FeatureServices>) -> Result<String, AppError> {
    // Channel scopes prereleases — stable users see only published
    // releases, beta/dev see prereleases too. Read here rather than
    // baking it into the updater module so the command stays the
    // single channel-aware caller.
    let channel = {
        let cfg = features.config.lock_or_recover();
        cfg.updates.channel.clone()
    };
    Ok(
        tauri::async_runtime::spawn_blocking(move || crate::updater::fetch_changelog(&channel))
            .await
            .map_err(|e| format!("Task error: {}", e))?
            .map_err(|e| format!("Fetch failed: {}", e))?,
    )
}

#[tauri::command]
pub async fn get_update_urls(
    features: State<'_, FeatureServices>,
) -> Result<serde_json::Value, AppError> {
    let channel = {
        let cfg = features.config.lock_or_recover();
        cfg.updates.channel.clone()
    };
    Ok(serde_json::json!({
        "channel": channel,
        "endpoint": crate::updater::endpoint_for_channel(&channel),
        "changelog_url": crate::updater::CHANGELOG_URL,
    }))
}

#[tauri::command]
pub async fn download_and_install_update(
    app: tauri::AppHandle,
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
    _acp: State<'_, AcpHandles>,
) -> Result<(), AppError> {
    // Prefer the Update cached from a previous check_for_update call —
    // it might carry channel-specific metadata the plugin would need to
    // re-fetch otherwise. Fall back to a fresh check if nothing cached.
    let update = {
        let mut slot = features.updater.pending_update.lock_or_recover();
        slot.take()
    };
    let update = if let Some(u) = update {
        u
    } else {
        let channel = {
            let cfg = features.config.lock_or_recover();
            cfg.updates.channel.clone()
        };
        crate::updater::plugin_check(&app, &channel)
            .await
            .map_err(|e| format!("Check failed: {}", e))?
            .ok_or_else(|| AppError::from("No update available"))?
    };

    // Stamp last_updated_version so the post-restart launch can show
    // the "welcome back after update" banner. Same story as the idle
    // path in start_update_loop.
    {
        let mut cfg = features.config.lock_or_recover();
        if let Ok(v) = features.updater.available_version.lock() {
            cfg.updates.last_updated_version = v.clone();
        }
        let _ = cfg.save();
    }

    // Write the resume marker so the relaunch restores the session.
    // Prefer floating's session (post-update banner shows the floating
    // window first); fall back to main's session if floating has none.
    let session_id = ui.window_sessions.lock().ok().and_then(|m| {
        m.get("floating")
            .cloned()
            .or_else(|| m.get("main").cloned())
    });
    crate::updater::persist_resume_marker(session_id.as_deref());
    // Tag this install as user-initiated so the post-install launch
    // shows the floating window with the celebration banner. The idle
    // path writes `Idle` instead, leaving the floating window hidden
    // until the user manually summons it.
    crate::updater::persist_install_source(crate::updater::InstallSource::Interactive);

    // Bubble the plugin error up verbatim — `plugin_download_and_install`
    // now classifies failures (signature / network / disk full /
    // permission / 403 / 404 / cancelled / other) and produces a
    // user-readable string. Wrapping with another "Install failed:"
    // here would double up by the time it reaches the UI.
    crate::updater::plugin_download_and_install(&app, update)
        .await
        .map_err(|e| AppError::from(e.to_string()))?;

    // On Windows the plugin called process::exit(0) inside
    // download_and_install and we never reached this line. On macOS
    // it returned cleanly after swapping the .app on disk; we exit
    // ourselves so launchd / the user relaunches into the new binary.
    // See plugin_download_and_install's doc-comment for the full
    // per-platform breakdown verified against the plugin source.
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn was_just_updated(features: State<'_, FeatureServices>) -> Result<bool, AppError> {
    let config = features.config.lock_or_recover();
    Ok(crate::updater::was_just_updated(&config))
}

#[tauri::command]
pub async fn clear_update_flag(features: State<'_, FeatureServices>) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    crate::updater::clear_update_flag(&mut config);
    Ok(config
        .save()
        .map_err(|e| format!("Failed to save: {}", e))?)
}

#[tauri::command]
pub async fn touch_floating_activity(features: State<'_, FeatureServices>) -> Result<(), AppError> {
    features.updater.touch_activity();
    Ok(())
}

/// Simulate a completed update by showing the update banner on the floating window.
pub fn simulate_update_complete(app: &tauri::AppHandle) {
    show_update_banner(app);
}

/// Show the floating window with an update celebration banner.
pub fn show_update_banner(app: &tauri::AppHandle) {
    if let Some(floating) = app.get_webview_window("floating") {
        let _ = floating.show();
        let _ = floating.set_focus();
    }
    let _ = app.emit(
        "show_floating_banner",
        serde_json::json!({
            "icon": "🎉",
            "text": "Kage has been updated!",
            "action_label": "View changelog →",
            "action_type": "settings",
            "action_data": "updates"
        }),
    );
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
    if let Some(floating) = app.get_webview_window("floating") {
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

// ---------------------------------------------------------------------------
// Agent auto-detection
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, Clone)]
pub struct DetectedAgent {
    /// Display name from the preset.
    pub name: String,
    /// Stable preset id (e.g. "kiro", "claude-code", "codex").
    pub preset_id: String,
    /// Absolute path to the binary that was found.
    pub path: String,
    /// Full spawn command (path + ACP args) ready to drop into config.
    pub spawn_command: String,
    /// Output of `<binary> --version` when it succeeded.
    pub version: Option<String>,
}

/// Static metadata for a preset, surfaced to the UI so the settings page
/// can render install instructions, auth hints, etc. without duplicating
/// the registry in JS.
#[derive(serde::Serialize, Clone)]
pub struct AgentPresetInfo {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub install_url: String,
    pub requires_auth: bool,
    pub auth_hint: Option<String>,
}

/// Connection-validation result returned by [`validate_agent_connection`].
/// `ok=false` and `issues` populated means we found problems the user
/// should see in the UI.
#[derive(serde::Serialize, Clone, Default)]
pub struct ConnectionIssues {
    /// The binary the user pointed at (resolved against PATH where
    /// applicable). `None` for remote connections.
    pub resolved_path: Option<String>,
    /// True when no issues were found.
    pub ok: bool,
    /// Issue codes the UI maps to friendly copy. Examples: "empty",
    /// "binary-not-found", "not-executable", "host-empty", "port-invalid".
    pub issues: Vec<String>,
}

/// List the known agent presets the UI can render in a "+ New
/// connection" dropdown.
#[tauri::command]
pub async fn list_agent_presets() -> Result<Vec<AgentPresetInfo>, AppError> {
    use crate::agent_presets::AgentKind;
    Ok(AgentKind::all()
        .iter()
        .map(|k| {
            let p = k.preset();
            AgentPresetInfo {
                id: p.id.to_string(),
                display_name: p.display_name.to_string(),
                description: p.description.to_string(),
                install_url: p.install_url.to_string(),
                requires_auth: p.requires_auth,
                auth_hint: p.auth_hint.map(|s| s.to_string()),
            }
        })
        .collect())
}

/// Validate a saved connection. For local connections, parses the
/// spawn_command, resolves the binary against PATH, and checks that it
/// exists. For remote connections, sanity-checks host/port. Cheap (no
/// process spawn) so it's safe to call on every render of the settings
/// page.
#[tauri::command]
pub async fn validate_agent_connection(
    mode: crate::config::AcpMode,
) -> Result<ConnectionIssues, AppError> {
    let mut out = ConnectionIssues::default();
    match mode {
        crate::config::AcpMode::Local { spawn_command } => {
            let trimmed = spawn_command.trim();
            if trimmed.is_empty() {
                out.issues.push("empty".to_string());
                return Ok(out);
            }
            // First whitespace-separated token is the binary; this
            // matches the transport's own parsing, so what we validate
            // is what would be spawned.
            let first = trimmed.split_whitespace().next().unwrap_or("");
            let resolved = resolve_binary_path(first);
            out.resolved_path = resolved.clone();
            if resolved.is_none() {
                out.issues.push("binary-not-found".to_string());
            }
            out.ok = out.issues.is_empty();
        }
        crate::config::AcpMode::Remote { host, port, .. } => {
            if host.trim().is_empty() {
                out.issues.push("host-empty".to_string());
            }
            if port == 0 {
                out.issues.push("port-invalid".to_string());
            }
            out.ok = out.issues.is_empty();
        }
    }
    Ok(out)
}

/// Resolve a binary token to an absolute path, mirroring how the
/// transport's `Command::new` resolves names. Absolute paths are
/// validated by `Path::exists`; bare names go through `where`/`which`.
fn resolve_binary_path(token: &str) -> Option<String> {
    let p = std::path::Path::new(token);
    if p.is_absolute() {
        return p.exists().then(|| token.to_string());
    }
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let mut command = std::process::Command::new(cmd);
    command.arg(token);
    // CREATE_NO_WINDOW: GUI subsystem processes spawning console
    // children inherit no console — Windows allocates a fresh one for
    // the child unless we suppress it, which flashes a DOS window.
    crate::os::configure_no_window(&mut command);
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = stdout.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

/// Search well-known locations for ACP-compatible agent binaries.
#[tauri::command]
pub async fn detect_agents() -> Result<Vec<DetectedAgent>, AppError> {
    Ok(tauri::async_runtime::spawn_blocking(detect_agents_sync)
        .await
        .map_err(|e| format!("Task error: {}", e))?)
}

fn detect_agents_sync() -> Vec<DetectedAgent> {
    use crate::agent_presets::detection_hints;

    let mut agents = Vec::new();
    let home = dirs::home_dir();

    for hint in detection_hints() {
        let preset = hint.kind.preset();
        for bin_name in hint.binary_names {
            let mut candidates: Vec<std::path::PathBuf> = Vec::new();

            // Windows-specific locations
            #[cfg(target_os = "windows")]
            {
                if let Ok(local) = std::env::var("LOCALAPPDATA") {
                    candidates.push(
                        std::path::PathBuf::from(&local)
                            .join("Toolbox")
                            .join("bin")
                            .join(format!("{}.exe", bin_name)),
                    );
                    candidates.push(
                        std::path::PathBuf::from(&local)
                            .join("Programs")
                            .join(format!("{}.exe", bin_name)),
                    );
                }
                if let Some(ref h) = home {
                    candidates.push(
                        h.join(".local")
                            .join("bin")
                            .join(format!("{}.exe", bin_name)),
                    );
                }
            }

            // macOS-specific locations
            #[cfg(target_os = "macos")]
            {
                if let Some(ref h) = home {
                    candidates.push(h.join(".local").join("bin").join(bin_name));
                    candidates.push(h.join(".toolbox").join("bin").join(bin_name));
                }
                candidates.push(std::path::PathBuf::from("/usr/local/bin").join(bin_name));
                candidates.push(std::path::PathBuf::from("/opt/homebrew/bin").join(bin_name));
            }

            // Linux-specific locations
            #[cfg(target_os = "linux")]
            {
                if let Some(ref h) = home {
                    candidates.push(h.join(".local").join("bin").join(bin_name));
                    candidates.push(h.join(".toolbox").join("bin").join(bin_name));
                    candidates.push(h.join("bin").join(bin_name));
                }
                candidates.push(std::path::PathBuf::from("/usr/local/bin").join(bin_name));
                candidates.push(std::path::PathBuf::from("/usr/bin").join(bin_name));
                candidates.push(std::path::PathBuf::from("/snap/bin").join(bin_name));
            }

            // Also check PATH via `which` / `where`. CREATE_NO_WINDOW
            // matters because the settings UI's Connection page calls
            // detect_agents during normal startup — without the flag
            // each `where` flashes a DOS window.
            let where_or_which = if cfg!(windows) { "where" } else { "which" };
            let mut where_cmd = std::process::Command::new(where_or_which);
            where_cmd.arg(bin_name);
            crate::os::configure_no_window(&mut where_cmd);
            if let Ok(output) = where_cmd.output() {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        let p = std::path::PathBuf::from(line.trim());
                        if !candidates.contains(&p) {
                            candidates.push(p);
                        }
                    }
                }
            }

            for path in candidates {
                if !path.exists() {
                    continue;
                }
                let path_str = path.to_string_lossy().to_string();

                // Skip duplicate detections of the same binary.
                if agents.iter().any(|a: &DetectedAgent| a.path == path_str) {
                    continue;
                }

                // Try to capture a version string. Skipped when the
                // preset declares no version_args (some adapters don't
                // implement --version).
                let version = if hint.version_args.is_empty() {
                    None
                } else {
                    let mut version_cmd = std::process::Command::new(&path);
                    version_cmd.args(hint.version_args);
                    // CREATE_NO_WINDOW: see comment above on the
                    // where/which call.
                    crate::os::configure_no_window(&mut version_cmd);
                    version_cmd.output().ok().and_then(|o| {
                        if o.status.success() {
                            let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            if !v.is_empty() {
                                Some(v)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                };

                let spawn_command = if hint.acp_args.is_empty() {
                    path_str.clone()
                } else {
                    format!("{} {}", path_str, hint.acp_args.join(" "))
                };

                agents.push(DetectedAgent {
                    name: preset.display_name.to_string(),
                    preset_id: preset.id.to_string(),
                    path: path_str,
                    spawn_command,
                    version,
                });
            }
        }
    }

    agents
}

// ---------------------------------------------------------------------------
// App Log commands
// ---------------------------------------------------------------------------

/// Write a log entry from the frontend.
#[tauri::command]
pub async fn app_log_write(level: String, source: String, msg: String) -> Result<(), AppError> {
    crate::app_log::log(&level, &source, &msg);
    Ok(())
}

/// Get all log entries.
#[tauri::command]
pub async fn app_log_get_entries() -> Result<Vec<crate::app_log::LogEntry>, AppError> {
    Ok(crate::app_log::get_entries())
}

/// Clear all log entries.
#[tauri::command]
pub async fn app_log_clear() -> Result<(), AppError> {
    crate::app_log::clear().map_err(|e| e.to_string())?;
    Ok(())
}

/// Get the log directory path (for "Open Logs Folder").
#[tauri::command]
pub async fn app_log_get_dir() -> Result<String, AppError> {
    Ok(crate::app_log::log_dir_string())
}

/// Dump thread CPU usage info to the log for debugging high-CPU issues.
/// Takes two snapshots 3 seconds apart to show which threads are actively
/// burning CPU, plus cumulative totals. Available via tray menu in debug mode.
#[tauri::command]
pub async fn dump_thread_info() -> Result<String, AppError> {
    #[cfg(target_os = "windows")]
    {
        // Run in a blocking thread since we sleep for 3 seconds
        let result = tauri::async_runtime::spawn_blocking(dump_thread_info_windows).await;
        match result {
            Ok(output) => Ok(output),
            Err(e) => Ok(format!("Thread dump task failed: {}", e)),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let result = tauri::async_runtime::spawn_blocking(dump_thread_info_macos).await;
        match result {
            Ok(output) => Ok(output),
            Err(e) => Ok(format!("Thread dump task failed: {}", e)),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Ok("Thread dump not implemented on this platform".to_string())
    }
}

#[cfg(target_os = "windows")]
/// (tid, delta_total, delta_user, delta_kernel, cum_total, cum_user, cum_kernel, name)
type ThreadDelta = (u32, f64, f64, f64, f64, f64, f64, String);

#[cfg(target_os = "windows")]
fn dump_thread_info_windows() -> String {
    use std::fmt::Write;

    let pid = std::process::id();
    let mut output = String::new();
    let _ = writeln!(output, "=== Thread Dump for PID {} ===", pid);

    // First snapshot
    let snap1 = snapshot_threads(pid);
    if snap1.is_empty() {
        let _ = writeln!(output, "Failed to snapshot threads");
        return output;
    }

    let _ = writeln!(output, "Sampling {} threads for 3 seconds...", snap1.len());
    std::thread::sleep(std::time::Duration::from_secs(3));

    // Second snapshot
    let snap2 = snapshot_threads(pid);

    // Compute deltas
    let mut deltas: Vec<ThreadDelta> = Vec::new();
    // (tid, delta_total, delta_user, delta_kernel, cum_total, cum_user, cum_kernel, name)
    for (tid, total2, user2, kernel2, name) in &snap2 {
        if let Some((_, total1, user1, kernel1, _)) = snap1.iter().find(|(t, _, _, _, _)| t == tid)
        {
            let dt = total2 - total1;
            let du = user2 - user1;
            let dk = kernel2 - kernel1;
            deltas.push((*tid, dt, du, dk, *total2, *user2, *kernel2, name.clone()));
        }
    }

    // Sort by delta descending
    deltas.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Active threads (delta > 0)
    let active: Vec<_> = deltas.iter().filter(|d| d.1 > 10.0).collect();
    if active.is_empty() {
        let _ = writeln!(
            output,
            "No threads used significant CPU in the 3s sample window."
        );
    } else {
        let _ = writeln!(output, "\n--- Active threads (CPU used in last 3s) ---");
        let _ = writeln!(
            output,
            "{:<8} {:<22} {:>10} {:>10} {:>10}  {:>12} {:>12} {:>12}",
            "TID", "Name", "Δ Total", "Δ User", "Δ Kernel", "Cum Total", "Cum User", "Cum Kernel"
        );
        let _ = writeln!(output, "{}", "-".repeat(105));
        for (tid, dt, du, dk, ct, cu, ck, name) in &active {
            let pct = dt / 3000.0 * 100.0; // % of one core
            let note = if pct > 80.0 {
                " ← SPINNING"
            } else if pct > 30.0 {
                " ← HOT"
            } else {
                ""
            };
            let display_name = if name.is_empty() { "-" } else { name.as_str() };
            let _ = writeln!(output, "{:<8} {:<22} {:>9.0}ms {:>9.0}ms {:>9.0}ms  {:>11.0}ms {:>11.0}ms {:>11.0}ms  ({:.0}% core){}",
                tid, display_name, dt, du, dk, ct, cu, ck, pct, note);
        }
    }

    // Top 10 by cumulative total
    let _ = writeln!(output, "\n--- All threads by cumulative CPU (top 10) ---");
    let _ = writeln!(
        output,
        "{:<8} {:<22} {:>12} {:>12} {:>12}",
        "TID", "Name", "Total(ms)", "User(ms)", "Kernel(ms)"
    );
    let _ = writeln!(output, "{}", "-".repeat(72));
    let mut by_cum = deltas.clone();
    by_cum.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    for (tid, _, _, _, ct, cu, ck, name) in by_cum.iter().take(10) {
        let display_name = if name.is_empty() { "-" } else { name.as_str() };
        let _ = writeln!(
            output,
            "{:<8} {:<22} {:>12.0} {:>12.0} {:>12.0}",
            tid, display_name, ct, cu, ck
        );
    }

    let _ = writeln!(output, "\n=== End Thread Dump ===");

    // Log it
    for line in output.lines() {
        info!("[ThreadDump] {}", line);
    }

    output
}

#[cfg(target_os = "windows")]
fn snapshot_threads(pid: u32) -> Vec<(u32, f64, f64, f64, String)> {
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::*;

    let mut threads = Vec::new();

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    let snapshot = match snapshot {
        Ok(h) => h,
        Err(e) => {
            error!("Failed to create thread snapshot: {}", e);
            return threads;
        }
    };

    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };

    unsafe {
        if Thread32First(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32OwnerProcessID == pid {
                    if let Ok(handle) = OpenThread(
                        THREAD_QUERY_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION,
                        false,
                        entry.th32ThreadID,
                    ) {
                        let mut creation = FILETIME::default();
                        let mut exit = FILETIME::default();
                        let mut kernel = FILETIME::default();
                        let mut user = FILETIME::default();

                        if GetThreadTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
                            .is_ok()
                        {
                            let kernel_ms = filetime_to_ms(&kernel);
                            let user_ms = filetime_to_ms(&user);

                            // Try to read the thread description (name)
                            let name = GetThreadDescription(handle)
                                .ok()
                                .and_then(|pwstr| {
                                    let s = pwstr.to_string().ok().unwrap_or_default();
                                    if s.is_empty() {
                                        None
                                    } else {
                                        Some(s)
                                    }
                                })
                                .unwrap_or_default();

                            threads.push((
                                entry.th32ThreadID,
                                kernel_ms + user_ms,
                                user_ms,
                                kernel_ms,
                                name,
                            ));
                        }
                        let _ = CloseHandle(handle);
                    }
                }

                entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
                if Thread32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }

    threads
}

#[cfg(target_os = "windows")]
fn filetime_to_ms(ft: &windows::Win32::Foundation::FILETIME) -> f64 {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
    // FILETIME is in 100-nanosecond intervals
    ticks as f64 / 10_000.0
}

// ---------------------------------------------------------------------------
// macOS thread dump via Mach APIs
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
/// (port, total_ms, user_ms, kernel_ms) — port is used to correlate threads across snapshots.
type MacThreadSnapshot = (u32, f64, f64, f64);

#[cfg(target_os = "macos")]
fn dump_thread_info_macos() -> String {
    use std::fmt::Write;

    let pid = std::process::id();
    let mut output = String::new();
    let _ = writeln!(output, "=== Thread Dump for PID {} ===", pid);

    let snap1 = snapshot_threads_macos();
    if snap1.is_empty() {
        let _ = writeln!(output, "Failed to snapshot threads");
        return output;
    }

    let _ = writeln!(output, "Sampling {} threads for 3 seconds...", snap1.len());
    std::thread::sleep(std::time::Duration::from_secs(3));

    let snap2 = snapshot_threads_macos();

    // Compute deltas by matching on thread port (stable identity across snapshots)
    // (port, delta_total, delta_user, delta_kernel, cum_total, cum_user, cum_kernel)
    let mut deltas: Vec<(u32, f64, f64, f64, f64, f64, f64)> = Vec::new();
    for (port2, total2, user2, kernel2) in &snap2 {
        if let Some((_, total1, user1, kernel1)) = snap1.iter().find(|(p, _, _, _)| p == port2) {
            let dt = total2 - total1;
            let du = user2 - user1;
            let dk = kernel2 - kernel1;
            deltas.push((*port2, dt, du, dk, *total2, *user2, *kernel2));
        }
    }

    // Sort by delta descending
    deltas.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Active threads (delta > 10ms in the 3s window)
    let active: Vec<_> = deltas.iter().filter(|d| d.1 > 10.0).collect();
    if active.is_empty() {
        let _ = writeln!(
            output,
            "No threads used significant CPU in the 3s sample window."
        );
    } else {
        let _ = writeln!(output, "\n--- Active threads (CPU used in last 3s) ---");
        let _ = writeln!(
            output,
            "{:<8} {:>10} {:>10} {:>10}  {:>12} {:>12} {:>12}",
            "Port", "Δ Total", "Δ User", "Δ Kernel", "Cum Total", "Cum User", "Cum Kernel"
        );
        let _ = writeln!(output, "{}", "-".repeat(85));
        for (port, dt, du, dk, ct, cu, ck) in &active {
            let pct = dt / 3000.0 * 100.0;
            let note = if pct > 80.0 {
                " ← SPINNING"
            } else if pct > 30.0 {
                " ← HOT"
            } else {
                ""
            };
            let _ = writeln!(
                output,
                "{:<8} {:>9.0}ms {:>9.0}ms {:>9.0}ms  {:>11.0}ms {:>11.0}ms {:>11.0}ms  ({:.0}% core){}",
                port, dt, du, dk, ct, cu, ck, pct, note
            );
        }
    }

    // Top 10 by cumulative total
    let _ = writeln!(output, "\n--- All threads by cumulative CPU (top 10) ---");
    let _ = writeln!(
        output,
        "{:<8} {:>12} {:>12} {:>12}",
        "Port", "Total(ms)", "User(ms)", "Kernel(ms)"
    );
    let _ = writeln!(output, "{}", "-".repeat(50));
    let mut by_cum = deltas.clone();
    by_cum.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    for (port, _, _, _, ct, cu, ck) in by_cum.iter().take(10) {
        let _ = writeln!(output, "{:<8} {:>12.0} {:>12.0} {:>12.0}", port, ct, cu, ck);
    }

    let _ = writeln!(output, "\n=== End Thread Dump ===");

    for line in output.lines() {
        info!("[ThreadDump] {}", line);
    }

    output
}

/// Snapshot all threads in the current task, returning (port, total_ms, user_ms, kernel_ms).
/// The port serves as a stable thread identity for correlating across snapshots.
#[cfg(target_os = "macos")]
fn snapshot_threads_macos() -> Vec<MacThreadSnapshot> {
    use std::mem;
    use std::ptr;

    // Mach types and constants
    type MachPort = u32;
    type KernReturn = i32;
    const KERN_SUCCESS: KernReturn = 0;
    const THREAD_BASIC_INFO: u32 = 3;
    const THREAD_BASIC_INFO_COUNT: u32 =
        (mem::size_of::<ThreadBasicInfo>() / mem::size_of::<i32>()) as u32;

    #[repr(C)]
    #[derive(Default)]
    struct TimeValue {
        seconds: i32,
        microseconds: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ThreadBasicInfo {
        user_time: TimeValue,
        system_time: TimeValue,
        cpu_usage: i32,
        policy: i32,
        run_state: i32,
        flags: i32,
        suspend_count: i32,
        sleep_time: i32,
    }

    extern "C" {
        // Use mach_task_self_ directly — the cached task port, avoids leaking send rights.
        static mach_task_self_: MachPort;
        fn task_threads(
            task: MachPort,
            thread_list: *mut *mut MachPort,
            thread_count: *mut u32,
        ) -> KernReturn;
        fn thread_info(
            thread: MachPort,
            flavor: u32,
            info: *mut i32,
            count: *mut u32,
        ) -> KernReturn;
        fn vm_deallocate(task: MachPort, address: usize, size: usize) -> KernReturn;
        fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
    }

    fn time_value_to_ms(tv: &TimeValue) -> f64 {
        tv.seconds as f64 * 1000.0 + tv.microseconds as f64 / 1000.0
    }

    let mut threads: Vec<MacThreadSnapshot> = Vec::new();

    unsafe {
        let task = mach_task_self_;
        let mut thread_list: *mut MachPort = ptr::null_mut();
        let mut thread_count: u32 = 0;

        if task_threads(task, &mut thread_list, &mut thread_count) != KERN_SUCCESS {
            return threads;
        }

        for i in 0..thread_count as isize {
            let thread_port = *thread_list.offset(i);
            let mut info: ThreadBasicInfo = mem::zeroed();
            let mut count = THREAD_BASIC_INFO_COUNT;

            if thread_info(
                thread_port,
                THREAD_BASIC_INFO,
                &mut info as *mut ThreadBasicInfo as *mut i32,
                &mut count,
            ) == KERN_SUCCESS
            {
                let user_ms = time_value_to_ms(&info.user_time);
                let kernel_ms = time_value_to_ms(&info.system_time);
                threads.push((thread_port, user_ms + kernel_ms, user_ms, kernel_ms));
            }

            mach_port_deallocate(task, thread_port);
        }

        // Free the thread list
        vm_deallocate(
            task,
            thread_list as usize,
            thread_count as usize * mem::size_of::<MachPort>(),
        );
    }

    threads
}

// --- Permission audit log commands ---

/// Read the most recent `limit` entries from the permission audit log,
/// newest-first. `limit` is clamped to 1..=2000 so a misbehaving UI
/// can't ask for an enormous slice.
#[tauri::command]
pub async fn get_permission_audit_log(
    limit: Option<usize>,
) -> Result<Vec<crate::permission_audit::AuditEntry>, AppError> {
    let n = limit.unwrap_or(500).clamp(1, 2000);
    Ok(crate::permission_audit::read_recent_default(n))
}

/// Clear the permission audit log. Intended for the "Clear log" button
/// in settings. Not destructive beyond the log itself — permissions
/// and grants are untouched.
#[tauri::command]
pub async fn clear_permission_audit_log() -> Result<(), AppError> {
    crate::permission_audit::clear_default()
        .map_err(|e| format!("Failed to clear audit log: {}", e))?;
    Ok(())
}

/// Return the filesystem path to the audit log so the UI can show it
/// to the user (e.g. "stored at: ~/.../permission-audit.jsonl") and
/// link to "open in file explorer".
#[tauri::command]
pub async fn get_permission_audit_log_path() -> Result<String, AppError> {
    Ok(crate::permission_audit::default_log_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Telemetry commands
//
// The Aptabase plugin is registered (or not) in main.rs based on the
// compile-time APTABASE_KEY. These commands let the UI surface the
// current telemetry state and change it without needing to poke at
// `config.telemetry` fields directly from the frontend.
// ---------------------------------------------------------------------------

/// Return the current telemetry snapshot for the Settings → Privacy UI.
#[tauri::command]
pub async fn get_telemetry_info(
    features: State<'_, FeatureServices>,
) -> Result<crate::telemetry::TelemetryInfo, AppError> {
    Ok(crate::telemetry::snapshot(&features.config))
}

/// Enable or disable anonymous telemetry. Applies immediately — any
/// subsequent calls to `telemetry::track()` respect the new value.
#[tauri::command]
pub async fn set_telemetry_enabled(
    app: tauri::AppHandle,
    features: State<'_, FeatureServices>,
    enabled: bool,
) -> Result<(), AppError> {
    // Track the toggle *before* applying it so an opt-out still sends a
    // single "telemetry_disabled" event — useful for measuring opt-out
    // rates. Opt-in cannot fire a pre-change event because telemetry
    // was off; we fire "telemetry_enabled" right after set_consent
    // instead.
    let was_enabled = features.config.lock_or_recover().telemetry.enabled;
    if was_enabled && !enabled {
        crate::telemetry::track(&app, "telemetry_disabled", None);
    }

    crate::telemetry::set_consent(&features.config, enabled);

    if !was_enabled && enabled {
        crate::telemetry::track(&app, "telemetry_enabled", None);
    }
    Ok(())
}

/// Generate a fresh anonymous install ID. The returned value is the new
/// ID (kept for UI display only — the plugin already uses it on the next
/// event).
#[tauri::command]
pub async fn reset_telemetry_install_id(
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    Ok(crate::telemetry::reset_install_id(&features.config))
}

/// Pass-through for the frontend to fire arbitrary telemetry events.
/// Gated by `telemetry.enabled` on the Rust side — calls from an opted-out
/// user become no-ops. Props are restricted to string/number values by
/// Aptabase's plugin, but we don't re-validate here because the plugin
/// already does.
///
/// Event names MUST be drawn from the known list in
/// `ui/js/shared/telemetry.js`. Unknown names are still accepted but
/// logged so accidental PII leakage through a misnamed event surfaces
/// during development.
#[tauri::command]
pub async fn telemetry_track(
    app: tauri::AppHandle,
    event: String,
    props: Option<serde_json::Value>,
) -> Result<(), AppError> {
    crate::telemetry::track(&app, &event, props);
    Ok(())
}

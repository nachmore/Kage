//! Tauri commands and app wiring for background agent calls
//! (docs/design/EXTENSION_AGENT_CALLS.md §3.5, §3.6, §5.7, WP-AC3).
//!
//! The engine is [`crate::agent_calls::AgentCallService`]; it lives in
//! `FeatureServices.agent_calls`. This module:
//!
//! - builds the service with the real agent behind it ([`build_service`]):
//!   every call runs on a strict ephemeral session
//!   (`ephemeral_session::run_with`);
//! - starts it from `setup()` ([`start_service`]) with hooks that write the
//!   audit log, send telemetry, emit `agent_calls_activity`, update the tray
//!   tooltip and read the panel's log rows, and stops it on shutdown
//!   ([`shutdown_service`]);
//! - exposes the nine commands. The five `agent_call_*` ones are reached by
//!   extensions holding `agent_background`; the sandbox host force-injects
//!   their `extensionId`. The four others are for first-party windows only
//!   (`null` in `COMMAND_CAPABILITIES`).
//!
//! Every command re-checks the four gates in Rust, inside the service.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Listener, Manager, Runtime, State};

use crate::acp_client::AcpClient;
use crate::agent_calls::{
    AccessPanel, AccessRecord, AcpBackend, ActivitySnapshot, AgentCallHooks, AgentCallResult,
    AgentCallService, AgentCallStatus, CallIdArgs, CallRecord, RequestDetail, Reservation,
    ReserveArgs, SendArgs, SessionOpener, SessionSpec, TurnInfo,
};
use crate::config::{AgentAllowance, Config};
use crate::ephemeral_session::{self, EphemeralOptions, StrictHooks};
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::permission_audit::{AuditEntry, AuditEvent};
use crate::state::FeatureServices;

/// Audit entries the panel scans for its rows (newest first).
const PANEL_LOG_SCAN: usize = 2000;

// --- Building and starting the service ------------------------------------------

/// Opens strict throwaway sessions for the engine: `run_with` with the
/// spec's options, so the engine itself doesn't name `ephemeral_session`.
pub fn session_opener(client: Arc<AcpClient>, config: Arc<Mutex<Config>>) -> SessionOpener {
    Arc::new(
        move |spec: SessionSpec, work: &mut dyn FnMut(&str) -> anyhow::Result<String>| {
            let options = EphemeralOptions {
                strict: spec.strict,
                require_connected: spec.require_connected,
                cwd: spec.cwd,
            };
            ephemeral_session::run_with(&client, &config, options, |session_id| work(session_id))
        },
    )
}

/// The service for `FeatureServices.agent_calls`. No I/O: built with the
/// rest of the managed state, before `setup()`.
pub fn build_service(config: Arc<Mutex<Config>>, client: Arc<AcpClient>) -> Arc<AgentCallService> {
    let open = session_opener(client.clone(), config.clone());
    let backend = Arc::new(AcpBackend::new(client, config.clone(), open));
    Arc::new(AgentCallService::new(config, backend))
}

/// Start the service: its hooks, the strict-session hooks the ACP
/// notification handler calls, the scheduler task, and a re-check of every
/// waiting and running call whenever the config changes (an agent switch,
/// a turned-off extension, a restored backup). Call once, from `setup()`.
pub fn start_service<R: Runtime>(app: &AppHandle<R>) {
    let Some(features) = app.try_state::<FeatureServices>() else {
        return;
    };
    let svc = features.agent_calls.clone();
    svc.start(Arc::new(AppHooks::new(app.clone())));
    ephemeral_session::set_strict_hooks(Some(strict_hooks()));
    let weak = Arc::downgrade(&svc);
    app.listen_any(crate::events::CONFIG_UPDATED, move |_| {
        if let Some(svc) = weak.upgrade() {
            // Off the emitting thread: the re-check reads manifests.
            tauri::async_runtime::spawn_blocking(move || svc.on_agent_changed());
        }
    });
}

/// Kage is quitting: stop new turns, cancel the running call and give it up
/// to 2 s to delete its session, write its audit entry and save the usage
/// file. Runs before the agent is disconnected (`graceful_shutdown`).
pub fn shutdown_service<R: Runtime>(app: &AppHandle<R>) {
    if let Some(features) = app.try_state::<FeatureServices>() {
        features.agent_calls.shutdown();
    }
    ephemeral_session::set_strict_hooks(None);
}

/// Kage didn't quit after [`shutdown_service`] (an update that failed to
/// install): take calls again, with the scheduler and the strict-session
/// hooks back.
pub fn resume_service<R: Runtime>(app: &AppHandle<R>) {
    let Some(features) = app.try_state::<FeatureServices>() else {
        return;
    };
    features.agent_calls.resume();
    ephemeral_session::set_strict_hooks(Some(strict_hooks()));
}

fn strict_hooks() -> StrictHooks {
    StrictHooks {
        tool_attempt: crate::agent_calls::note_tool_attempt,
        disconnected: crate::agent_calls::on_disconnect,
    }
}

// --- Hooks ---------------------------------------------------------------------------

/// What the engine sends out, with an app handle behind it.
struct AppHooks<R: Runtime> {
    app: AppHandle<R>,
    /// The extension the tray tooltip names now, to skip repeat updates.
    tooltip: Mutex<Option<String>>,
}

impl<R: Runtime> AppHooks<R> {
    fn new(app: AppHandle<R>) -> Self {
        Self {
            app,
            tooltip: Mutex::new(None),
        }
    }
}

impl<R: Runtime> AgentCallHooks for AppHooks<R> {
    fn audit_call(&self, record: &CallRecord) {
        crate::permission_audit::append(&AuditEntry::now(call_audit_event(record)));
    }

    fn audit_access(&self, record: &AccessRecord) {
        crate::permission_audit::append(&AuditEntry::now(access_audit_event(record)));
    }

    fn telemetry(&self, event: &str, props: serde_json::Value) {
        crate::telemetry::track(&self.app, event, Some(props));
    }

    fn activity(&self, snapshot: &ActivitySnapshot) {
        // First-party windows only: chat hosts, the launcher (the chip) and
        // Settings (the panel). Sandboxes never see Tauri events.
        crate::event_targets::emit_permission_audience(
            &self.app,
            crate::events::AGENT_CALLS_ACTIVITY,
            snapshot,
        );
        let name = tooltip_name(snapshot);
        let mut shown = self.tooltip.lock_or_recover();
        if *shown != name {
            crate::tray::set_agent_call_tooltip(&self.app, name.as_deref());
            *shown = name;
        }
    }

    fn log_rows(&self, extension_id: &str, limit: usize) -> Vec<serde_json::Value> {
        audit_rows(
            &crate::permission_audit::read_recent_default(PANEL_LOG_SCAN),
            extension_id,
            limit,
        )
    }
}

/// The extension the tray tooltip names: the one whose call runs.
fn tooltip_name(snapshot: &ActivitySnapshot) -> Option<String> {
    snapshot.running.first().map(|r| r.name.clone())
}

/// `AuditEvent::ExtensionAgentCall` for one call.
fn call_audit_event(r: &CallRecord) -> AuditEvent {
    AuditEvent::ExtensionAgentCall {
        extension_id: r.extension_id.clone(),
        call_id: r.call_id.clone(),
        purpose: r.purpose.clone(),
        label: r.label.clone(),
        agent: r.agent.clone(),
        outcome: r.outcome,
        cancel_reason: r.cancel_reason.and_then(|c| {
            serde_json::to_value(c)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
        }),
        prompt_chars: r.prompt_chars,
        response_chars: r.response_chars,
        duration_ms: r.duration_ms,
        redactions: r.redactions,
    }
}

/// `AuditEvent::ExtensionAgentAccessChanged` for a flip of the host switch.
fn access_audit_event(r: &AccessRecord) -> AuditEvent {
    AuditEvent::ExtensionAgentAccessChanged {
        extension_id: r.extension_id.clone(),
        enabled: r.enabled,
        agent: r.agent.clone(),
    }
}

/// The panel's rows: this extension's background AI entries, newest first
/// (as `entries` already are), at most `limit`.
fn audit_rows(entries: &[AuditEntry], extension_id: &str, limit: usize) -> Vec<serde_json::Value> {
    entries
        .iter()
        .filter(|e| match &e.event {
            AuditEvent::ExtensionAgentCall {
                extension_id: id, ..
            }
            | AuditEvent::ExtensionAgentAccessChanged {
                extension_id: id, ..
            } => id == extension_id,
            _ => false,
        })
        .take(limit)
        .filter_map(|e| serde_json::to_value(e).ok())
        .collect()
}

fn check_extension_id(extension_id: &str) -> Result<(), AppError> {
    crate::extensions::validate_extension_id(extension_id).map_err(|_| {
        AppError::keyed(
            ErrorKind::InvalidInput,
            "errors.agent_call.field_invalid",
            &[("field", "extension_id")],
        )
    })
}

// --- Commands extensions reach (capability `agent_background`) -------------------

/// Reserve a background call. Checks the gates, the purpose, the pauses,
/// the allowance and the queue; returns the call id and its place.
#[tauri::command]
pub async fn agent_call_reserve(
    extension_id: String,
    args: ReserveArgs,
    features: State<'_, FeatureServices>,
) -> Result<Reservation, AppError> {
    features.agent_calls.reserve(&extension_id, args)
}

/// Resolve when the call's turn starts (send within 10 s), or reject when
/// it is cancelled first.
#[tauri::command]
pub async fn agent_call_ready(
    extension_id: String,
    args: CallIdArgs,
    features: State<'_, FeatureServices>,
) -> Result<TurnInfo, AppError> {
    let svc = features.agent_calls.clone();
    svc.ready(&extension_id, args).await
}

/// Send the request on the call's turn and wait for the whole reply.
#[tauri::command]
pub async fn agent_call_send(
    extension_id: String,
    args: SendArgs,
    features: State<'_, FeatureServices>,
) -> Result<AgentCallResult, AppError> {
    let svc = features.agent_calls.clone();
    svc.send(&extension_id, args).await
}

/// Cancel a call at any stage. Idempotent; unknown ids are fine.
#[tauri::command]
pub async fn agent_call_cancel(
    extension_id: String,
    args: CallIdArgs,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    features.agent_calls.cancel(&extension_id, args)
}

/// This extension's own state: access, usage, queue and limits. Never the
/// agent's name, the user's idle state or other extensions' numbers.
#[tauri::command]
pub async fn agent_call_status(
    extension_id: String,
    features: State<'_, FeatureServices>,
) -> Result<AgentCallStatus, AppError> {
    Ok(features.agent_calls.status(&extension_id))
}

// --- Host-only commands (Settings, the launcher chip) -----------------------------

/// Everything the Background AI panel and the consent dialog show for one
/// extension, including the consent token to pass back when turning on.
#[tauri::command]
pub async fn get_extension_agent_access(
    extension_id: String,
    features: State<'_, FeatureServices>,
) -> Result<AccessPanel, AppError> {
    check_extension_id(&extension_id)?;
    Ok(features.agent_calls.access_panel(&extension_id))
}

/// Turn background AI on (with the token from `get_extension_agent_access`),
/// off, or change the allowance. Saves the config, writes an audit entry
/// when the switch flips, and tells every window (`config_updated`).
#[tauri::command]
pub async fn set_extension_agent_access<R: Runtime>(
    extension_id: String,
    enabled: bool,
    consent_token: Option<String>,
    allowance: Option<AgentAllowance>,
    features: State<'_, FeatureServices>,
    app: AppHandle<R>,
) -> Result<(), AppError> {
    check_extension_id(&extension_id)?;
    features
        .agent_calls
        .set_access(&extension_id, enabled, consent_token, allowance)?;
    if let Err(e) = app.emit(
        crate::events::CONFIG_UPDATED,
        crate::commands::i18n::ConfigUpdatedI18n::current(),
    ) {
        log::error!("Failed to emit config_updated: {}", e);
    }
    Ok(())
}

/// **Stop now**: cancel the running call and every waiting one, and pause
/// that extension (or, without an id, every extension) for an hour.
#[tauri::command]
pub async fn stop_extension_agent_calls(
    extension_id: Option<String>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    if let Some(id) = &extension_id {
        check_extension_id(id)?;
    }
    features.agent_calls.stop(extension_id.as_deref());
    Ok(())
}

/// The exact prompt and reply of one of this session's last calls. Memory
/// only: nothing older than this run can be shown.
#[tauri::command]
pub async fn get_extension_agent_request_detail(
    extension_id: String,
    call_id: String,
    features: State<'_, FeatureServices>,
) -> Result<RequestDetail, AppError> {
    features.agent_calls.request_detail(&extension_id, &call_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_calls::{AgentCallOutcome, CancelReason, RunningCall};

    fn record(outcome: AgentCallOutcome, reason: Option<CancelReason>) -> CallRecord {
        CallRecord {
            extension_id: "mindmap".into(),
            call_id: "c1".into(),
            purpose: "organize".into(),
            label: Some("Organize 3 notes".into()),
            agent: "Kiro".into(),
            outcome,
            cancel_reason: reason,
            prompt_chars: 1200,
            response_chars: 300,
            duration_ms: 4000,
            redactions: 2,
        }
    }

    #[test]
    fn a_call_record_becomes_its_audit_event() {
        let event = call_audit_event(&record(
            AgentCallOutcome::Cancelled,
            Some(CancelReason::AccessOff),
        ));
        let AuditEvent::ExtensionAgentCall {
            extension_id,
            call_id,
            purpose,
            label,
            agent,
            outcome,
            cancel_reason,
            prompt_chars,
            response_chars,
            duration_ms,
            redactions,
        } = event
        else {
            panic!("wrong variant");
        };
        assert_eq!(extension_id, "mindmap");
        assert_eq!(call_id, "c1");
        assert_eq!(purpose, "organize");
        assert_eq!(label.as_deref(), Some("Organize 3 notes"));
        assert_eq!(agent, "Kiro");
        assert_eq!(outcome, AgentCallOutcome::Cancelled);
        assert_eq!(cancel_reason.as_deref(), Some("access_off"));
        assert_eq!(
            (prompt_chars, response_chars, duration_ms, redactions),
            (1200, 300, 4000, 2)
        );

        let ok = call_audit_event(&record(AgentCallOutcome::Ok, None));
        assert!(matches!(
            ok,
            AuditEvent::ExtensionAgentCall {
                cancel_reason: None,
                outcome: AgentCallOutcome::Ok,
                ..
            }
        ));
        for (reason, text) in [
            (CancelReason::User, "user"),
            (CancelReason::Activity, "activity"),
            (CancelReason::Shutdown, "shutdown"),
            (CancelReason::Extension, "extension"),
        ] {
            let e = call_audit_event(&record(AgentCallOutcome::Cancelled, Some(reason)));
            let AuditEvent::ExtensionAgentCall { cancel_reason, .. } = e else {
                panic!("wrong variant");
            };
            assert_eq!(cancel_reason.as_deref(), Some(text));
        }
    }

    #[test]
    fn an_access_record_becomes_its_audit_event() {
        let event = access_audit_event(&AccessRecord {
            extension_id: "mindmap".into(),
            enabled: true,
            agent: Some("Kiro".into()),
        });
        assert_eq!(
            event,
            AuditEvent::ExtensionAgentAccessChanged {
                extension_id: "mindmap".into(),
                enabled: true,
                agent: Some("Kiro".into()),
            }
        );
    }

    #[test]
    fn panel_rows_are_this_extensions_background_ai_entries_only() {
        let at = |s: &str, e: AuditEvent| AuditEntry::at_time(s, e);
        let entries = vec![
            at(
                "2026-11-09T10:00:05.000Z",
                call_audit_event(&record(AgentCallOutcome::Ok, None)),
            ),
            at(
                "2026-11-09T10:00:04.000Z",
                AuditEvent::Denied {
                    tool: "shell".into(),
                    session_id: None,
                },
            ),
            at(
                "2026-11-09T10:00:03.000Z",
                AuditEvent::ExtensionAgentAccessChanged {
                    extension_id: "todo".into(),
                    enabled: true,
                    agent: None,
                },
            ),
            at(
                "2026-11-09T10:00:02.000Z",
                AuditEvent::ExtensionAgentAccessChanged {
                    extension_id: "mindmap".into(),
                    enabled: true,
                    agent: Some("Kiro".into()),
                },
            ),
            at(
                "2026-11-09T10:00:01.000Z",
                call_audit_event(&record(AgentCallOutcome::Timeout, None)),
            ),
        ];
        let rows = audit_rows(&entries, "mindmap", 20);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["event"], "extension_agent_call");
        assert_eq!(rows[0]["at"], "2026-11-09T10:00:05.000Z");
        assert_eq!(rows[1]["event"], "extension_agent_access_changed");
        assert_eq!(rows[2]["outcome"], "timeout");
        assert_eq!(audit_rows(&entries, "mindmap", 2).len(), 2);
        assert!(audit_rows(&entries, "weather", 20).is_empty());
    }

    #[test]
    fn the_tooltip_names_the_running_extension() {
        assert_eq!(tooltip_name(&ActivitySnapshot::default()), None);
        let snapshot = ActivitySnapshot {
            running: vec![RunningCall {
                extension_id: "mindmap".into(),
                name: "Mind Map".into(),
                purpose: "organize".into(),
                label: None,
                started_at: "2026-11-09T10:00:00+01:00".into(),
            }],
            waiting: 2,
        };
        assert_eq!(tooltip_name(&snapshot).as_deref(), Some("Mind Map"));
    }

    #[test]
    fn extension_ids_are_checked_before_host_commands_use_them() {
        assert!(check_extension_id("mindmap").is_ok());
        let e = check_extension_id("../evil").unwrap_err();
        assert_eq!(e.key, "errors.agent_call.field_invalid");
        assert!(matches!(e.kind, ErrorKind::InvalidInput));
        assert!(check_extension_id("").is_err());
    }
}

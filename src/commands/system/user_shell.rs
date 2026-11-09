//! Commands the user runs themselves: "Run it here" and "Run in Terminal"
//! on a shell block's sandbox notice (`ui/js/shared/shell-run-view.js`),
//! for a command the agent's own sandbox refused (Homebrew can't nest its
//! sandbox-exec profile inside the agent's, …). See docs/SECURITY_MODEL.md
//! § "Commands you run yourself" for the trust boundary.
//!
//! Every run starts from the user's click in Kage's confirm, which shows
//! the exact string that runs, where it runs, and what's risky about it;
//! nothing is remembered, so the next run needs its own click. None of
//! this is reachable from extensions (`extension-permissions.js` maps each
//! command to `null`, and their sandboxed iframes have no IPC of their
//! own), from Kage's other windows ([`caller_allowed`]), or from agent
//! output (which is rendered as text, never as a click).

pub mod runner;
#[cfg(test)]
mod tests;

use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::permission_audit::{AuditEntry, AuditEvent};
use crate::state::ChildProcesses;
use crate::window_labels;
use log::{info, warn};
use runner::{InvalidRun, RunControl, RunReport};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{State, WebviewWindow};

/// Windows that may run a command for the user: Kage's own chat windows
/// (`main`, `chat-<uuid>`) and the floating window — the ones that render
/// shell blocks. Settings, store, welcome, context menu and inline assist
/// never do.
pub fn caller_allowed(label: &str) -> bool {
    label == window_labels::MAIN
        || label == window_labels::FLOATING
        || window_labels::is_chat_label(label)
}

fn ensure_caller(label: &str) -> Result<(), AppError> {
    if caller_allowed(label) {
        Ok(())
    } else {
        warn!("User shell command refused for window '{label}'");
        Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.not_allowed",
            &[],
        ))
    }
}

fn invalid(e: InvalidRun) -> AppError {
    let max_kib = (runner::MAX_COMMAND_BYTES / 1024).to_string();
    match e {
        InvalidRun::EmptyCommand => {
            AppError::keyed(ErrorKind::Internal, "errors.user_shell.empty_command", &[])
        }
        InvalidRun::CommandTooLong => AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.command_too_long",
            &[("max_kib", &max_kib)],
        ),
        InvalidRun::NulInCommand => AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.invalid_command",
            &[],
        ),
        InvalidRun::BadRunId => {
            AppError::keyed(ErrorKind::Internal, "errors.user_shell.invalid_run_id", &[])
        }
        InvalidRun::ControlCharForTerminal => AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.terminal_control_chars",
            &[],
        ),
        InvalidRun::NoHome => {
            AppError::keyed(ErrorKind::Internal, "errors.user_shell.no_home", &[])
        }
        InvalidRun::CwdNotAbsolute(path) => AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.cwd_not_absolute",
            &[("path", &path)],
        ),
        InvalidRun::CwdMissing(path) => AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.cwd_missing",
            &[("path", &path)],
        ),
        InvalidRun::CwdNotDirectory(path) => AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.cwd_not_directory",
            &[("path", &path)],
        ),
    }
}

fn join_error(e: impl std::fmt::Display) -> AppError {
    AppError::internal(format!("user shell task failed: {e}"))
}

// --- One run per window --------------------------------------------------------

struct ActiveRun {
    run_id: String,
    /// The window showing the run (and its Stop button).
    window: String,
    cancel: Arc<AtomicBool>,
}

/// Slot keys of runs an agent asked for (`host_commands`) start with this;
/// a window label never does.
const AGENT_RUN_KEY: &str = "agent:";

/// The runs in progress: the user's own, one per window at most (keyed by
/// the window's label), and those an agent asked for through
/// `run_outside_sandbox` (keyed by run id, shown in a window too). Lives in
/// [`ChildProcesses`] so shutdown can stop them.
#[derive(Default)]
pub struct UserShellRuns {
    active: Mutex<HashMap<String, ActiveRun>>,
}

/// A claim on a run slot. Dropping it — when the run ends, however it
/// ends — frees the slot.
pub struct RunSlot {
    runs: Arc<UserShellRuns>,
    key: String,
    run_id: String,
    cancel: Arc<AtomicBool>,
}

impl RunSlot {
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        let mut active = self.runs.active.lock_or_recover();
        if active
            .get(&self.key)
            .is_some_and(|run| run.run_id == self.run_id)
        {
            active.remove(&self.key);
        }
    }
}

impl UserShellRuns {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert the run under `key` in `active` — the caller holds the lock
    /// it checked the slot under, so two runs can't both take it (the
    /// second would replace the first, which then nothing could stop).
    fn claim(
        self: &Arc<Self>,
        active: &mut HashMap<String, ActiveRun>,
        key: String,
        label: &str,
        run_id: &str,
    ) -> RunSlot {
        let cancel = Arc::new(AtomicBool::new(false));
        active.insert(
            key.clone(),
            ActiveRun {
                run_id: run_id.to_string(),
                window: label.to_string(),
                cancel: cancel.clone(),
            },
        );
        RunSlot {
            runs: self.clone(),
            key,
            run_id: run_id.to_string(),
            cancel,
        }
    }

    /// Claim `label`'s slot for the user's run `run_id`; `None` while it
    /// has one going.
    pub fn begin(self: &Arc<Self>, label: &str, run_id: &str) -> Option<RunSlot> {
        let mut active = self.active.lock_or_recover();
        if active.contains_key(label) {
            return None;
        }
        Some(self.claim(&mut active, label.to_string(), label, run_id))
    }

    /// Claim a slot for a run an agent asked for, shown in window `label`;
    /// `None` when `max` such runs are already going (or `run_id` is).
    pub fn begin_agent(self: &Arc<Self>, label: &str, run_id: &str, max: usize) -> Option<RunSlot> {
        let key = format!("{AGENT_RUN_KEY}{run_id}");
        let mut active = self.active.lock_or_recover();
        let agent_runs = active
            .keys()
            .filter(|k| k.starts_with(AGENT_RUN_KEY))
            .count();
        if agent_runs >= max || active.contains_key(&key) {
            return None;
        }
        Some(self.claim(&mut active, key, label, run_id))
    }

    /// Ask the run `run_id` shown in window `label` to stop. A window can
    /// only stop the runs it shows; returns whether that run was going.
    pub fn cancel(&self, label: &str, run_id: &str) -> bool {
        let active = self.active.lock_or_recover();
        match active
            .values()
            .find(|run| run.run_id == run_id && run.window == label)
        {
            Some(run) => {
                run.cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Stop every run `label` shows: its window closed or its page
    /// reloaded, so nothing is left to show them or their Stop buttons.
    /// Returns whether one was going.
    pub fn cancel_window(&self, label: &str) -> bool {
        let active = self.active.lock_or_recover();
        let mut any = false;
        for run in active.values().filter(|run| run.window == label) {
            run.cancel.store(true, Ordering::SeqCst);
            any = true;
        }
        any
    }

    /// Whether `label` has a run going.
    #[cfg(test)]
    pub fn is_running(&self, label: &str) -> bool {
        self.active.lock_or_recover().contains_key(label)
    }

    /// Shutdown: stop every run and wait up to `max` for them to end (each
    /// kills its process group; on macOS/Linux nothing else would, and a
    /// half-finished install is better ended with SIGTERM than with the
    /// SIGPIPE it would get once Kage's end of the pipes closes).
    pub fn cancel_all_and_wait(&self, max: Duration) {
        {
            let active = self.active.lock_or_recover();
            if active.is_empty() {
                return;
            }
            info!("Stopping {} user shell run(s) on shutdown", active.len());
            for run in active.values() {
                run.cancel.store(true, Ordering::SeqCst);
            }
        }
        let deadline = Instant::now() + max;
        while Instant::now() < deadline {
            if self.active.lock_or_recover().is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        warn!("User shell runs still stopping at shutdown");
    }
}

// --- Commands ------------------------------------------------------------------

/// Where a run would happen, for the confirm to show before the user
/// decides.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserShellRunContext {
    /// The directory it starts in (validated: an existing directory).
    pub cwd: String,
    /// The shell that runs it (`zsh`, `bash`, `PowerShell`, `cmd`).
    pub shell: String,
    /// Whether "Run in Terminal" can hand it to the terminal app (macOS).
    pub terminal: bool,
}

/// A finished run: where it ran and its [`RunReport`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserShellRunResult {
    pub run_id: String,
    pub cwd: String,
    pub shell: String,
    #[serde(flatten)]
    pub report: RunReport,
}

/// Resolve where a run would start (`cwd`, else the home directory) and
/// which shell would run it, without running anything. `shell` is the
/// Windows hint (`"cmd"`); ignored elsewhere.
#[tauri::command]
pub async fn user_shell_run_context<R: tauri::Runtime>(
    cwd: Option<String>,
    shell: Option<String>,
    window: WebviewWindow<R>,
) -> Result<UserShellRunContext, AppError> {
    ensure_caller(window.label())?;
    tauri::async_runtime::spawn_blocking(move || {
        let home = crate::paths::home_dir();
        let cwd = runner::resolve_cwd(cwd.as_deref(), home.as_deref()).map_err(invalid)?;
        Ok(UserShellRunContext {
            cwd: cwd.to_string_lossy().into_owned(),
            shell: runner::user_shell_invocation("", shell.as_deref()).label,
            terminal: cfg!(target_os = "macos"),
        })
    })
    .await
    .map_err(join_error)?
}

fn audit(
    command: &str,
    cwd: &Path,
    via: &str,
    outcome: &str,
    exit_code: Option<i32>,
    session_id: Option<String>,
) {
    crate::permission_audit::append(&AuditEntry::now(AuditEvent::UserRanCommand {
        command: crate::permission_audit::command_preview(command),
        cwd: cwd.to_string_lossy().into_owned(),
        via: via.to_string(),
        outcome: outcome.to_string(),
        exit_code,
        // An ACP session id is a short token; never let one bloat the log.
        session_id: session_id.map(|s| s.chars().take(128).collect()),
    }));
}

/// Why a run produced no report.
enum RunFailure {
    Invalid(InvalidRun),
    Spawn { cwd: PathBuf, error: String },
}

/// Run `command` — exactly as given — for the user, outside the agent's
/// sandbox: their login shell (`$SHELL -l -c --`; Windows: PowerShell, or
/// `cmd` when `shell` is `"cmd"`), in `cwd` (else their home directory),
/// stdin closed, with [`runner::RUN_TIMEOUT`]. Output streams to the
/// calling window as `user_shell_output` events tagged with `run_id` (the
/// window's own id for the run) and the last 64 KiB come back with the
/// exit status. One run at a time per window; `cancel_user_shell_command`
/// stops it. Only ever invoked from the confirm's Run button.
///
/// `command` and `run_id` must stay required: the mock-app sweep dispatches
/// every command with empty args, which must fail at deserialization.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri commands take state via parameters.
pub async fn run_user_shell_command<R: tauri::Runtime>(
    command: String,
    run_id: String,
    cwd: Option<String>,
    shell: Option<String>,
    session_id: Option<String>,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
    procs: State<'_, ChildProcesses>,
) -> Result<UserShellRunResult, AppError> {
    let label = window.label().to_string();
    ensure_caller(&label)?;
    runner::validate_run_id(&run_id).map_err(invalid)?;
    runner::validate_command(&command).map_err(invalid)?;
    let Some(slot) = procs.user_shell_runs.begin(&label, &run_id) else {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.busy",
            &[],
        ));
    };
    let control = RunControl {
        cancel: slot.cancel_flag(),
        timeout: runner::RUN_TIMEOUT,
    };

    let task_command = command.clone();
    let task_run_id = run_id.clone();
    let task_window = window.clone();
    let task_label = label.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        // Held for the whole run: the window's next run waits for this one.
        let _slot = slot;
        let home = crate::paths::home_dir();
        let cwd =
            runner::resolve_cwd(cwd.as_deref(), home.as_deref()).map_err(RunFailure::Invalid)?;
        let inv = runner::user_shell_invocation(&task_command, shell.as_deref());
        info!(
            "User shell run {}: started from window '{}': program '{}' ({} bytes) via {}",
            task_run_id,
            task_label,
            runner::program_label(&task_command),
            task_command.len(),
            inv.label
        );
        let emit_id = task_run_id.clone();
        let report = runner::run(&inv, &cwd, &control, move |text, skipped| {
            crate::event_targets::emit_to_self(
                &task_window,
                crate::events::USER_SHELL_OUTPUT,
                &serde_json::json!({ "runId": &emit_id, "text": text, "skipped": skipped }),
            );
        })
        .map_err(|e| RunFailure::Spawn {
            cwd: cwd.clone(),
            error: e.to_string(),
        })?;
        Ok((cwd, inv.label, report))
    })
    .await
    .map_err(join_error)?;

    match outcome {
        Ok((cwd, shell, report)) => {
            info!(
                "User shell run {}: {} (exit code {:?}, signal {:?}) after {} ms, {} bytes of output",
                run_id,
                report.outcome.as_str(),
                report.exit_code,
                report.signal,
                report.duration_ms,
                report.output_bytes
            );
            audit(
                &command,
                &cwd,
                "kage",
                report.outcome.as_str(),
                report.exit_code,
                session_id,
            );
            crate::telemetry::track(
                &app,
                "user_shell_run",
                Some(serde_json::json!({
                    "where": "kage",
                    "result": runner::outcome_bucket(&report),
                })),
            );
            Ok(UserShellRunResult {
                run_id,
                cwd: cwd.to_string_lossy().into_owned(),
                shell,
                report,
            })
        }
        Err(RunFailure::Invalid(e)) => Err(invalid(e)),
        Err(RunFailure::Spawn { cwd, error }) => {
            warn!("User shell run {run_id}: could not start: {error}");
            audit(&command, &cwd, "kage", "failed_to_start", None, session_id);
            crate::telemetry::track(
                &app,
                "user_shell_run",
                Some(serde_json::json!({ "where": "kage", "result": "spawn_error" })),
            );
            Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.user_shell.spawn_failed",
                &[("error", &error)],
            ))
        }
    }
}

/// Stop the calling window's run `run_id` (Stop button). Resolves to
/// whether it was running; the run itself then resolves as `cancelled`.
#[tauri::command]
pub async fn cancel_user_shell_command<R: tauri::Runtime>(
    run_id: String,
    window: WebviewWindow<R>,
    procs: State<'_, ChildProcesses>,
) -> Result<bool, AppError> {
    let label = window.label();
    ensure_caller(label)?;
    let stopped = procs.user_shell_runs.cancel(label, &run_id);
    if stopped {
        info!("User shell run {run_id}: stop requested");
    }
    Ok(stopped)
}

/// Hand `command` to the terminal app to run in `cwd` (else the home
/// directory), in a new window, as if typed there ("Run in Terminal").
/// macOS only (Terminal.app; the first time, macOS asks whether Kage may
/// control it); elsewhere it's refused and the notice copies instead.
/// Kage sees nothing of the run after handing it over. Terminal types the
/// command as keystrokes, so one holding a tab or another control
/// character (other than newlines) is refused
/// ([`runner::validate_terminal_command`]).
#[tauri::command]
pub async fn run_command_in_terminal<R: tauri::Runtime>(
    command: String,
    cwd: Option<String>,
    session_id: Option<String>,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    let label = window.label().to_string();
    ensure_caller(&label)?;
    runner::validate_terminal_command(&command).map_err(invalid)?;
    let task_command = command.clone();
    let (cwd, result) = tauri::async_runtime::spawn_blocking(move || {
        let home = crate::paths::home_dir();
        let cwd = runner::resolve_cwd(cwd.as_deref(), home.as_deref()).map_err(invalid)?;
        let result = crate::os::shell::run_in_terminal(&task_command, &cwd);
        Ok::<_, AppError>((cwd, result))
    })
    .await
    .map_err(join_error)??;

    use crate::os::shell::TerminalRunError;
    match result {
        Ok(()) => {
            info!(
                "User shell command handed to the terminal from window '{}': program '{}' ({} bytes)",
                label,
                runner::program_label(&command),
                command.len()
            );
            audit(&command, &cwd, "terminal", "opened", None, session_id);
            crate::telemetry::track(
                &app,
                "user_shell_run",
                Some(serde_json::json!({ "where": "terminal", "result": "opened" })),
            );
            Ok(())
        }
        Err(TerminalRunError::Unsupported) => Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.user_shell.terminal_unsupported",
            &[],
        )),
        Err(TerminalRunError::NotAuthorized) => {
            warn!("Run in Terminal: macOS refused Kage's Apple Event to Terminal");
            Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.user_shell.terminal_not_authorized",
                &[],
            ))
        }
        Err(TerminalRunError::Failed(detail)) => {
            warn!("Run in Terminal failed: {detail}");
            Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.user_shell.terminal_failed",
                &[("error", &detail)],
            ))
        }
    }
}

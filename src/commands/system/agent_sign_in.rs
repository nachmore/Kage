//! Signing in to the agent from Kage: when a prompt fails because the
//! agent is signed out, the error offers "Sign in", which runs the
//! agent's own login (`kiro-cli login`) in a terminal inside the window
//! that showed the error, with input enabled, so the user can pick a
//! sign-in method from its arrow-key menu. See docs/SECURITY_MODEL.md
//! § "Signing in to the agent".
//!
//! What runs is worked out here from the active connection in Config,
//! never from a request argument: the configured program must literally
//! be `kiro-cli` (basename), it runs with exactly `["login"]` in the
//! home directory, and only a Local connection qualifies (a Remote
//! agent's login lives on another machine). Only Kage's chat and floating
//! windows may start it ([`user_shell::caller_allowed`]); one sign-in
//! runs at a time in the whole app. Nothing the terminal shows or the
//! user types is logged, audited, sent to telemetry or returned.

use crate::agent_presets::{explicit_kind, AgentKind};
use crate::commands::system::user_shell::pty::{self, TerminalReport, TerminalSize, TerminalSpec};
use crate::commands::system::user_shell::{
    self, ensure_caller, invalid, join_error, runner, TerminalBusy,
};
use crate::config::{AcpMode, Config};
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::state::{ChildProcesses, FeatureServices};
use log::{info, warn};
use serde::Serialize;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};

/// A sign-in still going after this long is stopped: time to choose in
/// the menu plus the 10-minute life of an OIDC device code.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The only program the sign-in runs (by basename) and what it runs.
const SIGN_IN_PROGRAM: &str = "kiro-cli";
const SIGN_IN_ARGS: &[&str] = &["login"];

/// Sign-in terminals going at once in the whole app.
const MAX_SIGN_INS: usize = 1;

/// Why Kage can't sign in to the active agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    NoConnection,
    /// Not Kiro CLI, or not launched as a plain `kiro-cli` command.
    NotKiro,
    /// The agent runs on another machine; its login lives there.
    Remote,
}

impl Unavailable {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NoConnection => "no_connection",
            Self::NotKiro => "not_kiro",
            Self::Remote => "remote",
        }
    }

    fn error(self) -> AppError {
        match self {
            Self::Remote => {
                AppError::keyed(ErrorKind::NotPermitted, "errors.agent_sign_in.remote", &[])
            }
            Self::NoConnection | Self::NotKiro => AppError::keyed(
                ErrorKind::NotPermitted,
                "errors.agent_sign_in.unavailable",
                &[],
            ),
        }
    }
}

/// Whether a spawn command's program is Kiro CLI itself: its basename is
/// `kiro-cli` (any case; `kiro-cli.exe` on Windows). Stricter than
/// [`crate::agent_presets::detect_from_command`]: a wrapper
/// (`my-kiro-wrapper`), a sibling binary (`kiro-cli-chat`) or a launcher
/// (`wsl`, `sh -c`) is not it.
pub fn is_kiro_program(token: &str) -> bool {
    #[cfg(windows)]
    let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
    #[cfg(not(windows))]
    let base = token.rsplit('/').next().unwrap_or(token);
    let lower = base.to_ascii_lowercase();
    #[cfg(windows)]
    let lower = lower.strip_suffix(".exe").unwrap_or(&lower).to_string();
    lower == SIGN_IN_PROGRAM
}

/// The program token to sign in with, from the active connection: it
/// must be Kiro CLI, run locally, launched as `kiro-cli …`.
pub fn sign_in_target(cfg: &Config) -> Result<String, Unavailable> {
    let conn = cfg
        .acp
        .active_connection()
        .ok_or(Unavailable::NoConnection)?;
    if explicit_kind(cfg) != Some(AgentKind::Kiro) {
        return Err(Unavailable::NotKiro);
    }
    let spawn_command = match &conn.mode {
        AcpMode::Local { spawn_command } => spawn_command,
        AcpMode::Remote { .. } => return Err(Unavailable::Remote),
    };
    let program = crate::acp_client::tokenize_spawn_command(spawn_command)
        .into_iter()
        .next()
        .ok_or(Unavailable::NotKiro)?;
    if is_kiro_program(&program) {
        Ok(program)
    } else {
        Err(Unavailable::NotKiro)
    }
}

fn has_separator(token: &str) -> bool {
    if cfg!(windows) {
        token.contains(['/', '\\'])
    } else {
        token.contains('/')
    }
}

/// `candidate`, or on Windows also `candidate.exe` when it has no
/// extension (what PATHEXT resolution would find for a native binary).
fn executable_variants(candidate: PathBuf) -> Vec<PathBuf> {
    let mut out = vec![candidate.clone()];
    if cfg!(windows) && candidate.extension().is_none() {
        out.push(candidate.with_extension("exe"));
    }
    out
}

/// The absolute executable `token` names: an absolute path must itself
/// be executable (`is_exec`); a bare name is looked up in `path`
/// (absolute entries only, first match wins); a relative path with a
/// separator is refused.
pub fn resolve_program(
    token: &str,
    path: Option<&OsStr>,
    is_exec: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if token.is_empty() {
        return None;
    }
    let as_path = Path::new(token);
    if as_path.is_absolute() {
        return executable_variants(as_path.to_path_buf())
            .into_iter()
            .find(|p| is_exec(p));
    }
    if has_separator(token) {
        return None;
    }
    std::env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| executable_variants(dir.join(token)))
        .find(|p| is_exec(p))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    runner::is_executable_file(path)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// `s` as one POSIX shell word, for the Terminal hand-off.
fn posix_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | ':'));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// What the sign-in runs: `program login` in `home`, nothing else.
pub fn sign_in_spec(
    program: PathBuf,
    home: PathBuf,
    env: Vec<(String, String)>,
    size: TerminalSize,
) -> TerminalSpec {
    TerminalSpec {
        program,
        args: SIGN_IN_ARGS.iter().map(|a| a.to_string()).collect(),
        cwd: home,
        env,
        size: size.clamp(),
    }
}

/// The command as audited and shown: `<program> login`.
fn command_text(program: &Path) -> String {
    format!("{} {}", program.to_string_lossy(), SIGN_IN_ARGS.join(" "))
}

/// The login environment's PATH (else Kage's own). Can wait for the
/// first login-shell probe: blocking threads only.
fn search_path(overlay: &[(String, String)]) -> Option<std::ffi::OsString> {
    overlay
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.into())
        .or_else(|| std::env::var_os("PATH"))
}

fn not_found(token: &str) -> AppError {
    AppError::keyed(
        ErrorKind::Internal,
        "errors.agent_sign_in.not_found",
        &[("program", token)],
    )
}

fn no_home() -> AppError {
    invalid(runner::InvalidRun::NoHome)
}

/// Whether the window may offer "Sign in", and what it would run.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSignInContext {
    pub available: bool,
    /// Why not: `no_connection`, `not_kiro` or `remote`.
    pub reason: Option<&'static str>,
    /// The agent's display name (`Kiro CLI`).
    pub agent: Option<String>,
    /// What runs, as shown (`kiro-cli login`).
    pub command: Option<String>,
    /// Whether "Open in Terminal" can hand it to the terminal app (macOS).
    pub terminal: bool,
    pub timeout_minutes: u64,
}

/// [`AgentSignInContext`] for `cfg`.
pub fn sign_in_context(cfg: &Config) -> AgentSignInContext {
    let (available, reason) = match sign_in_target(cfg) {
        Ok(_) => (true, None),
        Err(why) => (false, Some(why.reason())),
    };
    AgentSignInContext {
        available,
        reason,
        agent: available.then(|| AgentKind::Kiro.preset().display_name.to_string()),
        command: available.then(|| format!("{SIGN_IN_PROGRAM} {}", SIGN_IN_ARGS.join(" "))),
        terminal: cfg!(target_os = "macos"),
        timeout_minutes: SIGN_IN_TIMEOUT.as_secs() / 60,
    }
}

/// A finished sign-in: its run id and how it ended (no output).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSignInResult {
    pub run_id: String,
    #[serde(flatten)]
    pub report: TerminalReport,
}

/// Whether the calling window can offer "Sign in" for the active agent.
/// Reads config only: never spawns anything or waits on the login shell.
#[tauri::command]
pub async fn agent_sign_in_context<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    features: State<'_, FeatureServices>,
) -> Result<AgentSignInContext, AppError> {
    ensure_caller(window.label())?;
    let cfg = features.config.lock_or_recover();
    Ok(sign_in_context(&cfg))
}

/// Why a sign-in produced no report.
enum StartFailure {
    NotFound(String),
    NoHome,
    Spawn {
        command: String,
        home: PathBuf,
        error: String,
    },
}

/// Run the active agent's login (`kiro-cli login`) in a terminal shown by
/// the calling window, until it exits, the user stops it, the window
/// closes, or [`SIGN_IN_TIMEOUT`]. What it prints goes to `on_output`
/// (`{ data, skipped }`) and only there; what the user types arrives
/// through `write_user_terminal`. Resolves with how it ended.
///
/// There is deliberately no command, program or argument parameter: what
/// runs comes from Config ([`sign_in_target`]). `session_id` is only
/// recorded in the audit log. `run_id` and `on_output` must stay
/// required: the mock-app sweep dispatches every command with empty
/// args, which must fail at deserialization.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri commands take state via parameters.
pub async fn start_agent_sign_in<R: tauri::Runtime>(
    run_id: String,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
    on_output: Channel<serde_json::Value>,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
    procs: State<'_, ChildProcesses>,
    features: State<'_, FeatureServices>,
) -> Result<AgentSignInResult, AppError> {
    let label = window.label().to_string();
    ensure_caller(&label)?;
    runner::validate_run_id(&run_id).map_err(invalid)?;
    let token = {
        let cfg = features.config.lock_or_recover();
        sign_in_target(&cfg).map_err(Unavailable::error)?
    };
    let slot = match procs
        .user_shell_runs
        .begin_terminal(&label, &run_id, MAX_SIGN_INS)
    {
        Ok(slot) => slot,
        Err(TerminalBusy::Window) => {
            return Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.user_shell.busy",
                &[],
            ))
        }
        Err(TerminalBusy::App) => {
            return Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.agent_sign_in.busy",
                &[],
            ))
        }
    };
    let control = runner::RunControl {
        cancel: slot.cancel_flag(),
        timeout: SIGN_IN_TIMEOUT,
    };
    let runs = procs.user_shell_runs.clone();
    let task_run_id = run_id.clone();
    let task_label = label.clone();
    let task_session = session_id.clone();
    let size = TerminalSize { cols, rows }.clamp();

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        // Held for the whole run: dropping it detaches the terminal's IO.
        let _slot = slot;
        let home = crate::paths::home_dir().ok_or(StartFailure::NoHome)?;
        // May wait for the first login-shell probe (fine: blocking thread).
        let overlay = crate::os::login_env::overlay(&["TERM", "COLORTERM"]);
        let program = resolve_program(&token, search_path(&overlay).as_deref(), is_executable)
            .ok_or_else(|| StartFailure::NotFound(token.clone()))?;
        let command = command_text(&program);
        let spec = sign_in_spec(program, home.clone(), pty::terminal_env(overlay), size);
        info!(
            "Agent sign-in run {}: started from window '{}' (program '{}')",
            task_run_id, task_label, SIGN_IN_PROGRAM
        );
        user_shell::audit(
            &command,
            &home,
            "sign_in",
            "started",
            None,
            task_session.clone(),
        );
        let attach_id = task_run_id.clone();
        let attach_label = task_label.clone();
        let report = pty::run_terminal(
            spec,
            &control,
            move |io| runs.attach_terminal_io(&attach_id, &attach_label, io),
            move |text, skipped| {
                // The window may be gone; the run goes on until it's stopped.
                let _ = on_output.send(serde_json::json!({ "data": text, "skipped": skipped }));
            },
        )
        .map_err(|e| StartFailure::Spawn {
            command: command.clone(),
            home: home.clone(),
            error: e.to_string(),
        })?;
        Ok((command, home, report))
    })
    .await
    .map_err(join_error)?;

    match outcome {
        Ok((command, home, report)) => {
            info!(
                "Agent sign-in run {}: {} (exit code {:?}, signal {:?}) after {} ms",
                run_id,
                report.outcome.as_str(),
                report.exit_code,
                report.signal,
                report.duration_ms
            );
            user_shell::audit(
                &command,
                &home,
                "sign_in",
                report.outcome.as_str(),
                report.exit_code,
                session_id,
            );
            crate::telemetry::track(
                &app,
                "agent_sign_in",
                Some(serde_json::json!({
                    "where": "kage",
                    "result": runner::outcome_bucket_parts(
                        report.outcome,
                        report.exit_code,
                        report.signal.is_some(),
                    ),
                })),
            );
            Ok(AgentSignInResult { run_id, report })
        }
        Err(StartFailure::NoHome) => Err(no_home()),
        Err(StartFailure::NotFound(token)) => {
            warn!("Agent sign-in run {run_id}: the agent's program wasn't found");
            Err(not_found(&token))
        }
        Err(StartFailure::Spawn {
            command,
            home,
            error,
        }) => {
            warn!("Agent sign-in run {run_id}: could not start: {error}");
            user_shell::audit(
                &command,
                &home,
                "sign_in",
                "failed_to_start",
                None,
                session_id,
            );
            crate::telemetry::track(
                &app,
                "agent_sign_in",
                Some(serde_json::json!({ "where": "kage", "result": "spawn_error" })),
            );
            Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.agent_sign_in.spawn_failed",
                &[("command", &command), ("error", &error)],
            ))
        }
    }
}

/// Hand the same sign-in (`<kiro-cli> login`, resolved the same way) to
/// the terminal app instead (macOS; elsewhere refused). Kage sees nothing
/// of it after handing it over. `run_id` is required only so the mock-app
/// sweep can't open Terminal.app; it is validated and otherwise unused.
#[tauri::command]
pub async fn agent_sign_in_in_terminal<R: tauri::Runtime>(
    run_id: String,
    session_id: Option<String>,
    window: WebviewWindow<R>,
    app: tauri::AppHandle<R>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let label = window.label().to_string();
    ensure_caller(&label)?;
    runner::validate_run_id(&run_id).map_err(invalid)?;
    let token = {
        let cfg = features.config.lock_or_recover();
        sign_in_target(&cfg).map_err(Unavailable::error)?
    };
    let (command, home, result) = tauri::async_runtime::spawn_blocking(move || {
        let home = crate::paths::home_dir().ok_or_else(no_home)?;
        let overlay = crate::os::login_env::overlay(&[]);
        let program = resolve_program(&token, search_path(&overlay).as_deref(), is_executable)
            .ok_or_else(|| not_found(&token))?;
        let typed = format!(
            "{} {}",
            posix_quote(&program.to_string_lossy()),
            SIGN_IN_ARGS.join(" ")
        );
        runner::validate_terminal_command(&typed).map_err(invalid)?;
        let result = crate::os::shell::run_in_terminal(&typed, &home);
        Ok::<_, AppError>((command_text(&program), home, result))
    })
    .await
    .map_err(join_error)??;

    use crate::os::shell::TerminalRunError;
    match result {
        Ok(()) => {
            info!("Agent sign-in handed to the terminal from window '{label}'");
            user_shell::audit(&command, &home, "terminal", "opened", None, session_id);
            crate::telemetry::track(
                &app,
                "agent_sign_in",
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
            warn!("Agent sign-in in Terminal: macOS refused Kage's Apple Event to Terminal");
            Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.user_shell.terminal_not_authorized",
                &[],
            ))
        }
        Err(TerminalRunError::Failed(detail)) => {
            warn!("Agent sign-in in Terminal failed: {detail}");
            Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.user_shell.terminal_failed",
                &[("error", &detail)],
            ))
        }
    }
}

#[cfg(test)]
mod tests;

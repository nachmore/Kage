//! Signing in to the agent from Kage: when a prompt fails because the
//! agent is signed out, the error offers "Sign in", which runs the
//! agent's own login (`kiro-cli login`) in a terminal inside the window
//! that showed the error, with input enabled, so the user can pick a
//! sign-in method from its arrow-key menu. See docs/SECURITY_MODEL.md
//! § "Signing in to the agent".
//!
//! What runs is worked out here from the active connection in Config,
//! never from a request argument: the configured program must literally
//! be `kiro-cli` (basename), it runs as exactly `<kiro-cli> login`
//! ([`SIGN_IN_ARGS`]) in the home directory, with the fixed
//! [`sign_in_env_for`] on top on Linux and Windows, and only a Local
//! connection qualifies (a Remote
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
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{State, WebviewWindow};

/// A sign-in still going after this long is stopped: time to choose in
/// the menu plus the 10-minute life of an OIDC device code.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The only program the sign-in runs (by basename).
const SIGN_IN_PROGRAM: &str = "kiro-cli";

/// The sign-in's arguments, on every platform: fixed, never from a
/// request. No `--use-device-flow`: on macOS that would lock out an
/// organization that has turned device-flow sign-in off for nothing (the
/// browser sign-in is safe there); Linux and Windows get the device flow
/// from [`sign_in_env_for`] instead.
pub const SIGN_IN_ARGS: &[&str] = &["login"];

/// The platform a sign-in runs on, as far as how kiro-cli opens the
/// browser goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInPlatform {
    /// kiro-cli opens a URL with `NSWorkspace openURL` (LaunchServices):
    /// launchd starts the browser, never as the login's descendant.
    MacOs,
    /// Windows: kiro-cli would run `cmd /c start <url>`, so a browser it
    /// starts is its descendant, in Kage's Job Object and in the login's
    /// `taskkill /T` tree.
    Windows,
    /// Linux and the other Unixes: kiro-cli would run `xdg-open` (or
    /// `wslview`) inside the terminal's session and process group.
    Linux,
}

impl SignInPlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// Whether kiro-cli is told it runs remotely, so it signs in with the
    /// device flow and only prints the URL and code ([`sign_in_env_for`]).
    pub fn device_flow_only(self) -> bool {
        !matches!(self, Self::MacOs)
    }
}

/// What makes kiro-cli's `is_remote()` true. Upstream
/// (aws/amazon-q-developer-cli at 15cc8f3c,
/// `crates/chat-cli/src/util/system_info/mod.rs:177-180`:
/// `in_ssh() || in_wsl() || is_remote_fake()`; `util/env_var.rs:38-40`:
/// `Q_FAKE_IS_REMOTE` is set at all) reads `Q_FAKE_IS_REMOTE`; kiro-cli
/// 3.3.0 carries `KIRO_FAKE_IS_REMOTE` beside it in its table of
/// `Q_*`/`KIRO_*` names, so both are set. Only presence counts; `1` for
/// readability.
pub const REMOTE_ENV: &[(&str, &str)] = &[("KIRO_FAKE_IS_REMOTE", "1"), ("Q_FAKE_IS_REMOTE", "1")];

/// The environment the login gets on top of the terminal's, per platform:
/// fixed, never from a request.
///
/// On Linux and Windows, [`REMOTE_ENV`]. Upstream's `LoginArgs::execute`
/// (`crates/chat-cli/src/cli/user.rs:132`) takes the device flow whenever
/// `is_remote()`, and `try_device_authorization` (`:300-305`) then only
/// prints "Open this URL: …" next to the code, never calling
/// `open_url_async`; the browser (PKCE) branch, the only other caller, is
/// the `else` of `:132` (kiro-cli 3.3.0 has the same branch: its binary
/// carries "Remote environment detected - using device flow
/// authentication"). Without it kiro-cli
/// starts the browser itself (`util/open.rs`): `xdg-open` (`:38-46`)
/// inside the terminal's session on Linux, where the group hang-up after
/// the login exits would close it, and `cmd /c start` (`:27-35`) on
/// Windows, where the browser would be in the login's `taskkill /T` tree
/// (and Kage's kill-on-close Job Object, which it inherits). With it the
/// user clicks the printed link and Kage opens it from its own process
/// (`open_user_terminal_link`), like every other link Kage opens. macOS
/// gets nothing: LaunchServices starts the browser outside the login, so
/// the usual browser sign-in stays.
pub fn sign_in_env_for(platform: SignInPlatform) -> &'static [(&'static str, &'static str)] {
    if platform.device_flow_only() {
        REMOTE_ENV
    } else {
        &[]
    }
}

/// The command a user types into their own terminal to sign in, when
/// Kage can't do it for them (device flow turned off by their
/// organization): the configured program and `login`. Their terminal has
/// no [`REMOTE_ENV`], so kiro-cli signs in through the browser there.
/// Quoted only when it has to be: POSIX single quotes, or PowerShell's
/// call operator on Windows (Windows Terminal's default shell).
pub fn manual_command_for(platform: SignInPlatform, program: &str) -> String {
    let windows = platform == SignInPlatform::Windows;
    let plain = !program.is_empty()
        && program.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '/' | '.' | '_' | '-' | '+' | ':')
                // A POSIX shell would read a backslash as an escape.
                || (c == '\\' && windows)
        });
    let args = SIGN_IN_ARGS.join(" ");
    if plain {
        format!("{program} {args}")
    } else if windows {
        format!("& '{}' {args}", program.replace('\'', "''"))
    } else {
        format!("{} {args}", posix_quote(program))
    }
}

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

/// What the sign-in runs on `platform`: `program` with [`SIGN_IN_ARGS`]
/// in `home`, with `env` and then [`sign_in_env_for`] (which `env` can't
/// override or drop), nothing else.
pub fn sign_in_spec_for(
    platform: SignInPlatform,
    program: PathBuf,
    home: PathBuf,
    env: Vec<(String, String)>,
    size: TerminalSize,
) -> TerminalSpec {
    let fixed = sign_in_env_for(platform);
    let mut env: Vec<(String, String)> = env
        .into_iter()
        .filter(|(k, _)| !REMOTE_ENV.iter().any(|(r, _)| k.eq_ignore_ascii_case(r)))
        .collect();
    env.extend(fixed.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    TerminalSpec {
        program,
        args: SIGN_IN_ARGS.iter().map(|a| a.to_string()).collect(),
        cwd: home,
        env,
        size: size.clamp(),
    }
}

/// [`sign_in_spec_for`] this platform.
pub fn sign_in_spec(
    program: PathBuf,
    home: PathBuf,
    env: Vec<(String, String)>,
    size: TerminalSize,
) -> TerminalSpec {
    sign_in_spec_for(SignInPlatform::current(), program, home, env, size)
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
    /// Whether the sign-in here can only use the device flow (Linux and
    /// Windows, [`sign_in_env_for`]): an organization that turned it off
    /// has to sign in from the user's own terminal.
    pub device_flow_only: bool,
    /// What to type into their own terminal then ([`manual_command_for`]).
    pub manual_command: Option<String>,
    pub timeout_minutes: u64,
}

/// [`AgentSignInContext`] for `cfg` on `platform`.
pub fn sign_in_context_for(platform: SignInPlatform, cfg: &Config) -> AgentSignInContext {
    let (target, reason) = match sign_in_target(cfg) {
        Ok(program) => (Some(program), None),
        Err(why) => (None, Some(why.reason())),
    };
    let available = target.is_some();
    AgentSignInContext {
        available,
        reason,
        agent: available.then(|| AgentKind::Kiro.preset().display_name.to_string()),
        command: available.then(|| format!("{SIGN_IN_PROGRAM} {}", SIGN_IN_ARGS.join(" "))),
        terminal: platform == SignInPlatform::MacOs,
        device_flow_only: platform.device_flow_only(),
        manual_command: target.map(|program| manual_command_for(platform, &program)),
        timeout_minutes: SIGN_IN_TIMEOUT.as_secs() / 60,
    }
}

/// [`sign_in_context_for`] this platform.
pub fn sign_in_context(cfg: &Config) -> AgentSignInContext {
    sign_in_context_for(SignInPlatform::current(), cfg)
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
/// and only there, as `{ seq, data }` messages small enough that Tauri
/// delivers each to this window by `eval` and never through its shared
/// channel cache ([`pty::CHANNEL_DIRECT_JSON_LIMIT`]); the window
/// acknowledges each with `ack_user_terminal` once written, and stops
/// getting more when it falls [`pty::MAX_UNACKED_BYTES`] behind. What the
/// user types arrives through `write_user_terminal`. Resolves with how it
/// ended.
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
    on_output: Channel,
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
    let control = slot.control(SIGN_IN_TIMEOUT);
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
            move |json| {
                // Sent as the exact JSON pty measured against the limit.
                // The window may be gone; the run goes on until it's
                // stopped (with nothing acknowledged, it sends no more).
                let _ = on_output.send(InvokeResponseBody::Json(json));
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

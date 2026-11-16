//! The window side of a live terminal run (the sign-in terminal,
//! `commands::system::agent_sign_in`): typing into it, resizing it,
//! acknowledging its output, stopping it, and opening a link it printed. Each command answers only
//! the window that shows the run; to any other window a live run looks
//! exactly like an unknown id. What is typed, printed or opened is never
//! logged (a sign-in prints device codes and URLs with tokens).

use super::pty::{InputError, TerminalSize, MAX_INPUT_BYTES};
use super::{ensure_caller, invalid, runner, TerminalLookup};
use crate::error::{AppError, ErrorKind};
use crate::state::ChildProcesses;
use log::{info, warn};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

/// Longest link the terminal may open.
pub const MAX_LINK_BYTES: usize = 2048;

fn not_found() -> AppError {
    AppError::keyed(ErrorKind::Internal, "errors.user_terminal.not_found", &[])
}

/// The caller's own live terminal run `run_id`.
fn owned_terminal(
    procs: &ChildProcesses,
    label: &str,
    run_id: &str,
) -> Result<Arc<super::pty::TerminalIo>, AppError> {
    ensure_caller(label)?;
    runner::validate_run_id(run_id).map_err(invalid)?;
    match procs.user_shell_runs.lookup_terminal(label, run_id) {
        Ok(io) => Ok(io),
        Err(TerminalLookup::Missing) => Err(not_found()),
        Err(TerminalLookup::OtherWindow(owner)) => {
            warn!(
                "User terminal {run_id}: window '{label}' refused, it belongs to window '{owner}'"
            );
            Err(not_found())
        }
    }
}

/// Whether `url` may be opened from the terminal: an `https` URL with a
/// host, at most [`MAX_LINK_BYTES`]. Returns it parsed.
pub fn checked_link(url: &str) -> Option<url::Url> {
    if url.is_empty() || url.len() > MAX_LINK_BYTES {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str().is_none_or(str::is_empty) {
        return None;
    }
    Some(parsed)
}

/// Type `data` into the caller's terminal run `run_id` (keystrokes or a
/// paste, control characters included: arrow keys and Ctrl+C are
/// legitimate input). At most [`MAX_INPUT_BYTES`]; never blocks.
#[tauri::command]
pub async fn write_user_terminal<R: tauri::Runtime>(
    run_id: String,
    data: String,
    window: WebviewWindow<R>,
    procs: State<'_, ChildProcesses>,
) -> Result<(), AppError> {
    let label = window.label().to_string();
    let io = owned_terminal(&procs, &label, &run_id)?;
    match io.write(data.as_bytes()) {
        Ok(()) => Ok(()),
        Err(InputError::TooLong) => Err(AppError::keyed(
            ErrorKind::InvalidInput,
            "errors.user_terminal.input_too_long",
            &[("max_kib", &(MAX_INPUT_BYTES / 1024).to_string())],
        )),
        Err(InputError::Full) => Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.user_terminal.input_full",
            &[],
        )),
        Err(InputError::Closed) => Err(not_found()),
    }
}

/// Tell the caller's terminal run `run_id` its view is now `cols` × `rows`
/// (clamped to 20..=500 × 4..=200).
#[tauri::command]
pub async fn resize_user_terminal<R: tauri::Runtime>(
    run_id: String,
    cols: u16,
    rows: u16,
    window: WebviewWindow<R>,
    procs: State<'_, ChildProcesses>,
) -> Result<(), AppError> {
    let label = window.label().to_string();
    let io = owned_terminal(&procs, &label, &run_id)?;
    // A terminal that just closed can't be resized; nothing to report.
    let _ = io.resize(TerminalSize { cols, rows }.clamp());
    Ok(())
}

/// The caller's terminal run `run_id` has written its output message
/// `seq` and every one before it (xterm's write callback): the run may
/// send more. Output stops after [`super::pty::MAX_UNACKED_BYTES`] that
/// aren't acknowledged, so a window that stalls holds the program back
/// instead of making Kage buffer.
#[tauri::command]
pub async fn ack_user_terminal<R: tauri::Runtime>(
    run_id: String,
    seq: u64,
    window: WebviewWindow<R>,
    procs: State<'_, ChildProcesses>,
) -> Result<(), AppError> {
    let label = window.label().to_string();
    owned_terminal(&procs, &label, &run_id)?.ack(seq);
    Ok(())
}

/// Stop the caller's terminal run `run_id` (Stop button). Resolves to
/// whether it was going; the run itself then resolves as `cancelled`.
#[tauri::command]
pub async fn stop_user_terminal<R: tauri::Runtime>(
    run_id: String,
    window: WebviewWindow<R>,
    procs: State<'_, ChildProcesses>,
) -> Result<bool, AppError> {
    let label = window.label();
    ensure_caller(label)?;
    runner::validate_run_id(&run_id).map_err(invalid)?;
    let stopped = procs.user_shell_runs.cancel(label, &run_id);
    if stopped {
        info!("User terminal {run_id}: stop requested");
    }
    Ok(stopped)
}

/// Open a link the caller's live terminal run `run_id` printed (the user
/// clicked it) in the default browser: `https` only, at most
/// [`MAX_LINK_BYTES`]. Logs only the host: a sign-in URL can carry a code.
#[tauri::command]
pub async fn open_user_terminal_link<R: tauri::Runtime>(
    run_id: String,
    url: String,
    window: WebviewWindow<R>,
    procs: State<'_, ChildProcesses>,
) -> Result<(), AppError> {
    let label = window.label().to_string();
    owned_terminal(&procs, &label, &run_id)?;
    let Some(link) = checked_link(&url) else {
        return Err(AppError::keyed(
            ErrorKind::InvalidInput,
            "errors.user_terminal.bad_link",
            &[],
        ));
    };
    let host = link.host_str().unwrap_or("").to_string();
    info!("User terminal {run_id}: opening a link (host {host})");
    let target = link.to_string();
    tauri::async_runtime::spawn_blocking(move || crate::os::shell::open_url_outside_job(&target))
        .await
        .map_err(super::join_error)?
        .map_err(|e| {
            warn!("User terminal {run_id}: could not open the link (host {host}): {e}");
            AppError::keyed(ErrorKind::Internal, "errors.user_terminal.open_failed", &[])
        })
}

#[cfg(test)]
mod tests {
    use super::checked_link;

    #[test]
    fn only_https_links_open() {
        assert!(checked_link("https://view.awsapps.com/start/#/device?user_code=AB").is_some());
        for bad in [
            "",
            "http://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "kage://install/x",
            "https://",
            "not a url",
        ] {
            assert!(checked_link(bad).is_none(), "{bad}");
        }
        let long = format!("https://example.com/{}", "a".repeat(2048));
        assert!(checked_link(&long).is_none());
    }
}

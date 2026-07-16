//! Script execution for the computer-control MCP `run_script` tool.
//!
//! The agent backend (kiro-cli) ships its own inline-shell tool, but the
//! model constantly trips over quoting/escaping when it inlines a multi-line
//! PowerShell (or bash heredoc) into a single `-Command` string. This tool
//! sidesteps that entirely: the script arrives as a JSON string argument
//! (JSON handles all escaping), Kage writes it verbatim to a temp file, and
//! runs the file with the platform interpreter — no shell-quoting in the loop.
//!
//! Permissioning is deliberately NOT handled here. Like every other
//! computer-control tool, `run_script` is registered with kiro-cli under
//! `autoApprove: []` (see `mcp_registration`), so the agent sends a
//! `session/request_permission` before each call. That routes through Kage's
//! single, native tool-permission system (config policy → in-app modal →
//! audit log) as `Running: @kage-computer-control/run_script` — the same gate,
//! UI, and allow-always/deny flow as every other tool. This module just runs
//! the script once execution has been authorised upstream; adding a second
//! confirmation here would be a redundant, inconsistent security surface.

use std::io::Write;
use std::process::Command;
use std::time::Duration;

/// A supported script language. Determines the interpreter, the temp-file
/// extension, and how the file is handed to the interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptLang {
    /// PowerShell (`powershell`/`pwsh`). Windows-first, but pwsh works
    /// cross-platform if installed.
    PowerShell,
    /// POSIX shell script run via `bash` (falls back to `sh`).
    Bash,
}

impl ScriptLang {
    /// Parse the `lang` argument. Accepts a few common aliases so the model
    /// isn't punished for saying "ps1" or "shell".
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "powershell" | "pwsh" | "ps" | "ps1" => Some(Self::PowerShell),
            "bash" | "sh" | "shell" | "zsh" => Some(Self::Bash),
            _ => None,
        }
    }

    fn file_extension(self) -> &'static str {
        match self {
            Self::PowerShell => "ps1",
            Self::Bash => "sh",
        }
    }
}

/// Outcome of a `run_script` invocation. Serialised to JSON for the model.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScriptResult {
    /// Process exit code, or None if the process was killed (e.g. timeout).
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// True when the run exceeded `timeout_ms` and was killed.
    pub timed_out: bool,
}

/// Default cap so a runaway script can't wedge the MCP server forever.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Upper bound the caller can request. Beyond this the model should be
/// launching a background process, not blocking a tool call.
pub const MAX_TIMEOUT_MS: u64 = 600_000;

/// The result of attempting a run: either it executed (with a `ScriptResult`),
/// or it couldn't start because this platform has no interpreter for the
/// requested language. (Authorization refusals are handled upstream by the
/// permission system and never reach here — see the module docs.)
pub enum RunOutcome {
    Ran(ScriptResult),
    /// No interpreter available for the requested language on this platform.
    Unsupported(String),
}

/// Write `script` to a temp file and execute it with the platform interpreter.
///
/// `timeout_ms` is clamped to `[1, MAX_TIMEOUT_MS]`, defaulting to
/// `DEFAULT_TIMEOUT_MS` when `None`. The temp file is removed on the way out
/// regardless of outcome. Assumes the call has already been authorised by the
/// permission system (see module docs) — there is no confirmation here.
pub fn run_script(lang: ScriptLang, script: &str, timeout_ms: Option<u64>) -> RunOutcome {
    let timeout = Duration::from_millis(
        timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(1, MAX_TIMEOUT_MS),
    );

    // Resolve the interpreter first — no point writing a file for a language
    // we can't run on this platform.
    let (program, mut pre_args) = match interpreter_for(lang) {
        Some(v) => v,
        None => {
            return RunOutcome::Unsupported(format!(
                "No interpreter available for {:?} on this platform",
                lang
            ));
        }
    };

    let temp_path = match write_temp_script(lang, script) {
        Ok(p) => p,
        Err(e) => {
            return RunOutcome::Ran(ScriptResult {
                exit_code: None,
                stdout: String::new(),
                stderr: format!("Failed to write temp script: {}", e),
                timed_out: false,
            });
        }
    };

    pre_args.push(temp_path.to_string_lossy().to_string());
    let outcome = execute_with_timeout(program, &pre_args, timeout);

    // Best-effort cleanup — a leftover temp script is harmless but untidy.
    let _ = std::fs::remove_file(&temp_path);

    RunOutcome::Ran(outcome)
}

/// Interpreter program + leading args for a language on the current platform.
/// Returns None when nothing suitable exists.
fn interpreter_for(lang: ScriptLang) -> Option<(&'static str, Vec<String>)> {
    match lang {
        ScriptLang::PowerShell => {
            // `-File <path>` runs the script file directly — no quoting of the
            // script body, which is the whole point. `-NoProfile` keeps user
            // profile scripts from perturbing output or slowing startup.
            #[cfg(target_os = "windows")]
            {
                Some((
                    "powershell",
                    vec!["-NoProfile".to_string(), "-File".to_string()],
                ))
            }
            // On macOS/Linux PowerShell is `pwsh` if the user installed it.
            // We can't know without probing; hand off the name and let the
            // spawn fail with a clear error if it's absent.
            #[cfg(not(target_os = "windows"))]
            {
                Some(("pwsh", vec!["-NoProfile".to_string(), "-File".to_string()]))
            }
        }
        ScriptLang::Bash => {
            #[cfg(target_os = "windows")]
            {
                // No reliable system bash on stock Windows. Steer the model to
                // PowerShell rather than guessing at a WSL/Git-bash path.
                None
            }
            #[cfg(not(target_os = "windows"))]
            {
                Some(("bash", vec![]))
            }
        }
    }
}

/// Write the script to a uniquely-named temp file with the right extension.
fn write_temp_script(lang: ScriptLang, script: &str) -> std::io::Result<std::path::PathBuf> {
    let mut path = std::env::temp_dir();
    // Uniqueness without pulling in a uuid dep: pid + a process-lifetime
    // counter. Two concurrent runs in one MCP process still get distinct
    // names; across processes the pid differs.
    let n = next_counter();
    path.push(format!(
        "kage-run-{}-{}.{}",
        std::process::id(),
        n,
        lang.file_extension()
    ));

    let mut file = std::fs::File::create(&path)?;
    // A UTF-8 BOM makes Windows PowerShell treat the file as UTF-8; without
    // it, non-ASCII in the script can be mangled under some code pages.
    if lang == ScriptLang::PowerShell {
        file.write_all(&[0xEF, 0xBB, 0xBF])?;
    }
    file.write_all(script.as_bytes())?;
    file.flush()?;
    Ok(path)
}

/// Process-lifetime monotonic counter for temp-file names.
fn next_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Spawn the interpreter, capture stdout/stderr, and enforce `timeout`.
/// On timeout the child is killed and `timed_out` is set.
fn execute_with_timeout(program: &str, args: &[String], timeout: Duration) -> ScriptResult {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());
    crate::os::configure_no_window(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ScriptResult {
                exit_code: None,
                stdout: String::new(),
                stderr: format!("Failed to launch '{}': {}", program, e),
                timed_out: false,
            };
        }
    };

    // Read pipes on separate threads so a child that fills one pipe's buffer
    // while we block on the other can't deadlock us.
    let stdout_handle = child.stdout.take().map(spawn_reader);
    let stderr_handle = child.stderr.take().map(spawn_reader);

    let start = std::time::Instant::now();
    let mut timed_out = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break None,
        }
    };

    let stdout = stdout_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stderr = stderr_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();

    ScriptResult {
        exit_code,
        stdout,
        stderr,
        timed_out,
    }
}

/// Drain a child pipe to a String on its own thread.
fn spawn_reader<R: std::io::Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = r.read_to_string(&mut buf);
        buf
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_common_aliases() {
        assert_eq!(
            ScriptLang::parse("powershell"),
            Some(ScriptLang::PowerShell)
        );
        assert_eq!(ScriptLang::parse("PWSH"), Some(ScriptLang::PowerShell));
        assert_eq!(ScriptLang::parse("ps1"), Some(ScriptLang::PowerShell));
        assert_eq!(ScriptLang::parse("bash"), Some(ScriptLang::Bash));
        assert_eq!(ScriptLang::parse(" sh "), Some(ScriptLang::Bash));
        assert_eq!(ScriptLang::parse("zsh"), Some(ScriptLang::Bash));
        assert_eq!(ScriptLang::parse("ruby"), None);
        assert_eq!(ScriptLang::parse(""), None);
    }

    #[test]
    fn extensions_match_language() {
        assert_eq!(ScriptLang::PowerShell.file_extension(), "ps1");
        assert_eq!(ScriptLang::Bash.file_extension(), "sh");
    }

    #[test]
    fn counter_is_monotonic_within_process() {
        let a = next_counter();
        let b = next_counter();
        assert!(b > a);
    }

    #[test]
    fn write_temp_script_roundtrips_body() {
        let body = "echo hello\n# a comment with \"quotes\" and 'apostrophes'";
        let path = write_temp_script(ScriptLang::Bash, body).expect("write");
        let read = std::fs::read_to_string(&path).expect("read");
        assert_eq!(read, body, "bash script written verbatim, no BOM");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn powershell_temp_script_has_utf8_bom() {
        let path = write_temp_script(ScriptLang::PowerShell, "Write-Output 'hi'").expect("write");
        let bytes = std::fs::read(&path).expect("read");
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "PS1 gets a UTF-8 BOM");
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn bash_available_off_windows() {
        assert!(interpreter_for(ScriptLang::Bash).is_some());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn bash_unavailable_on_windows() {
        assert!(interpreter_for(ScriptLang::Bash).is_none());
    }
}

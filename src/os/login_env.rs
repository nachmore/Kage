//! Login-shell `PATH` inheritance for spawned agent processes.
//!
//! GUI apps on macOS (launched from Finder / Dock / launchd) and many Linux
//! desktop sessions inherit a minimal `PATH` such as
//! `/usr/bin:/bin:/usr/sbin:/sbin`. Anything the user installed under
//! `~/.toolbox/bin`, `/opt/homebrew/bin`, `~/.cargo/bin`, … is invisible.
//! The ACP agent (e.g. `kiro-cli acp`) inherits that `PATH`, and so do the
//! MCP servers it spawns — so an `mcp.json` entry like
//! `"command": "builder-mcp"` silently fails to start even though it works
//! from a terminal.
//!
//! We fix this once: ask the user's login shell for its `PATH`, cache it,
//! and merge it into the environment of every agent process we spawn.
//! Everything here is best-effort — on any failure we fall back to the
//! inherited `PATH` unchanged.

use std::sync::OnceLock;

/// Markers bracketing the PATH in the shell's stdout, so rc-file noise
/// (motd, `echo`s, prompt escape codes) can't corrupt the result.
#[cfg(unix)]
const START_MARKER: &str = "__KAGE_PATH_START__";
#[cfg(unix)]
const END_MARKER: &str = "__KAGE_PATH_END__";

/// Upper bound on how long we'll wait for the login shell. Heavy rc files
/// can be slow; a hung one (e.g. waiting on a prompt) must not block the
/// agent spawn forever.
#[cfg(unix)]
const SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

static LOGIN_PATH: OnceLock<Option<String>> = OnceLock::new();

/// `PATH` reported by the user's login shell, resolved once and cached.
/// `None` when it can't be determined (non-unix, no `$SHELL`, timeout, …).
pub fn login_shell_path() -> Option<&'static str> {
    LOGIN_PATH.get_or_init(resolve_login_shell_path).as_deref()
}

/// Resolve the login-shell `PATH` on a background thread so the first
/// agent spawn doesn't pay the shell start-up cost.
pub fn prewarm() {
    std::thread::Builder::new()
        .name("kage-login-path".into())
        .spawn(|| {
            let _ = login_shell_path();
        })
        .ok();
}

/// The `PATH` to give a spawned agent: the login-shell entries first (they
/// reflect the user's chosen precedence), then any inherited entries the
/// login shell didn't list. `None` means "leave `PATH` as inherited".
pub fn agent_path() -> Option<String> {
    let login = login_shell_path()?;
    let current = std::env::var("PATH").unwrap_or_default();
    Some(merge_paths(login, &current))
}

/// Merge two colon-separated `PATH` strings, preserving order and dropping
/// empty and duplicate entries. `primary` entries win precedence.
pub fn merge_paths(primary: &str, secondary: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    primary
        .split(':')
        .chain(secondary.split(':'))
        .filter(|entry| !entry.is_empty() && seen.insert(*entry))
        .collect::<Vec<_>>()
        .join(":")
}

/// Pull the PATH out of shell stdout delimited by the start/end markers.
#[cfg(unix)]
fn extract_marked_path(stdout: &str) -> Option<String> {
    let start = stdout.find(START_MARKER)? + START_MARKER.len();
    let len = stdout[start..].find(END_MARKER)?;
    let path = stdout[start..start + len].trim();
    (!path.is_empty()).then(|| path.to_string())
}

#[cfg(unix)]
fn resolve_login_shell_path() -> Option<String> {
    use log::{info, warn};
    use std::io::Read;
    use std::process::{Command, Stdio};

    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());

    // `-i -l` loads both login (.zprofile/.bash_profile) and interactive
    // (.zshrc/.bashrc) config — toolbox and most installers append to the
    // interactive one. `printf` avoids a trailing newline / echo quirks.
    let script = format!("printf '%s%s%s' '{START_MARKER}' \"$PATH\" '{END_MARKER}'");
    let mut child = match Command::new(&shell)
        .args(["-i", "-l", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            warn!("login PATH: failed to run {shell}: {e}");
            return None;
        }
    };

    // Read stdout on a helper thread so a chatty rc file can't deadlock us
    // on a full pipe while we poll for exit.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });

    let deadline = std::time::Instant::now() + SHELL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                warn!(
                    "login PATH: {shell} timed out after {SHELL_TIMEOUT:?}; using inherited PATH"
                );
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(e) => {
                warn!("login PATH: waiting on {shell} failed: {e}");
                return None;
            }
        }
    }

    let output = reader.join().ok()?;
    let path = extract_marked_path(&output);
    match &path {
        Some(p) => info!("login PATH resolved via {shell}: {p}"),
        None => warn!("login PATH: no PATH in {shell} output; using inherited PATH"),
    }
    path
}

#[cfg(not(unix))]
fn resolve_login_shell_path() -> Option<String> {
    // Windows GUI processes already inherit the full user/system PATH.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_puts_primary_first_and_dedupes() {
        let merged = merge_paths(
            "/Users/me/.toolbox/bin:/opt/homebrew/bin:/usr/bin",
            "/usr/bin:/bin:/usr/sbin:/sbin",
        );
        assert_eq!(
            merged,
            "/Users/me/.toolbox/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }

    #[test]
    fn merge_drops_empty_entries() {
        assert_eq!(merge_paths("::/a::/b:", ":/b:/c:"), "/a:/b:/c");
        assert_eq!(merge_paths("", ""), "");
    }

    #[cfg(unix)]
    #[test]
    fn extract_ignores_rc_noise() {
        let out =
            format!("Welcome!\n\x1b]0;title\x07{START_MARKER}/x/bin:/usr/bin{END_MARKER}bye\n");
        assert_eq!(
            extract_marked_path(&out).as_deref(),
            Some("/x/bin:/usr/bin")
        );
    }

    #[cfg(unix)]
    #[test]
    fn extract_rejects_missing_or_empty() {
        assert_eq!(extract_marked_path("no markers here"), None);
        assert_eq!(
            extract_marked_path(&format!("{START_MARKER}{END_MARKER}")),
            None
        );
        assert_eq!(extract_marked_path(&format!("{START_MARKER}/a")), None);
    }

    #[cfg(unix)]
    #[test]
    fn resolves_a_real_login_path() {
        // Every unix CI/dev box has a shell; the result must at least
        // contain a standard system dir.
        let path = resolve_login_shell_path().expect("login shell PATH");
        assert!(
            path.split(':').any(|p| p == "/usr/bin" || p == "/bin"),
            "{path}"
        );
    }
}

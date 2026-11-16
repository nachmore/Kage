// Cross-platform shell operations for opening URLs and paths

use anyhow::Result;

/// `url` as it may appear in a log: `scheme://host[:port]` and nothing
/// else. No path (some carry secrets: a reset token, a signed link, a
/// session id), query, fragment or userinfo (OAuth codes, device codes
/// and tokens live there), since `app.jsonl` gets attached to bug
/// reports. A URL with no host (`mailto:`, `data:`, `javascript:`,
/// `file:`) logs as its scheme only; one that doesn't parse, as its
/// length.
pub fn url_for_log(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return format!("<unparsed URL, {} bytes>", url.len());
    };
    match parsed.host_str() {
        Some(host) if !host.is_empty() => {
            let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
            format!("{}://{host}{port}", parsed.scheme())
        }
        _ => format!("{}:", parsed.scheme()),
    }
}

/// Open a URL in the default browser. Platform code logs it only through
/// [`url_for_log`].
pub fn open_url(url: &str) -> Result<()> {
    crate::os::platform::shell::open_url_impl(url)
}

/// Open a file or directory path with the default application
pub fn open_path(path: &str) -> Result<()> {
    crate::os::platform::shell::open_path_impl(path)
}

/// Reveal a file in the native file manager, selecting it
pub fn reveal_in_file_manager(path: &str) -> Result<()> {
    crate::os::platform::shell::reveal_in_file_manager_impl(path)
}

/// Open a file in the default editor
pub fn open_in_editor(path: &str) -> Result<()> {
    crate::os::platform::shell::open_in_editor_impl(path)
}

/// Spawn a process with elevated (admin/root) privileges.
/// Windows: ShellExecuteW with "runas" verb.
/// macOS/Linux: pkexec wrapper.
pub fn spawn_elevated(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    crate::os::platform::shell::spawn_elevated_impl(program, args)
}

/// Get the program and arguments for a well-known system command.
/// Returns (program, args) appropriate for the current platform.
pub fn system_command(cmd: &str) -> (&'static str, Vec<&'static str>) {
    crate::os::platform::shell::system_command_impl(cmd)
}

/// Every way to run a well-known system command, in order of preference:
/// `system_command`'s program first, then the platform's alternatives for
/// when that one isn't installed. Only the terminal has alternatives: no
/// single terminal launcher exists on every Linux distribution, and
/// Windows Terminal is missing from many Windows 10 installs.
pub fn system_command_candidates(cmd: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut candidates = vec![system_command(cmd)];
    candidates.extend(crate::os::platform::shell::system_command_fallbacks_impl(
        cmd,
    ));
    candidates
}

/// Why [`run_in_terminal`] didn't run the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalRunError {
    /// No safe way to hand the terminal a command on this platform.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported,
    /// macOS refused the Apple Event: the user said no to (or never saw)
    /// "Kage wants to control Terminal" (Privacy & Security → Automation).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NotAuthorized,
    /// The terminal couldn't be reached; the detail is English, for logs.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Failed(String),
}

/// Open a new window of the user's terminal app and run `command` in it,
/// in `cwd`, as if they had typed it: their interactive shell, a real TTY
/// (so `sudo` can ask for a password), output in front of them. Blocks
/// until the terminal accepted it — on macOS that can include the
/// one-time "Kage wants to control Terminal" prompt — so call it from a
/// blocking thread.
///
/// macOS only (Terminal.app via Apple Events, the command passed as an
/// argument, never spliced into script source). Windows and Linux return
/// [`TerminalRunError::Unsupported`]: Windows Terminal re-parses `;` in its
/// command line and Linux terminals each quote differently, so there the
/// command is only copied for the user to paste.
pub fn run_in_terminal(command: &str, cwd: &std::path::Path) -> Result<(), TerminalRunError> {
    crate::os::platform::shell::run_in_terminal_impl(command, cwd)
}

#[cfg(test)]
mod tests {
    use super::url_for_log;

    #[test]
    fn logged_urls_keep_only_scheme_host_and_port() {
        let cases = [
            (
                "https://view.awsapps.com/start/#/device?user_code=ABCD-EFGH",
                "https://view.awsapps.com",
            ),
            (
                "https://accounts.spotify.com/authorize?client_id=x&code_challenge=SECRET&state=s",
                "https://accounts.spotify.com",
            ),
            (
                "https://user:pa55@example.com:8443/a/b?token=SECRET#frag",
                "https://example.com:8443",
            ),
            (
                "http://127.0.0.1:8080/callback?code=SECRET",
                "http://127.0.0.1:8080",
            ),
            // Secrets in the path: reset tokens, signed links, session ids.
            (
                "https://example.com/reset/SECRET-TOKEN/confirm",
                "https://example.com",
            ),
            (
                "https://hooks.slack.com/services/T000/B000/SECRET",
                "https://hooks.slack.com",
            ),
            ("https://example.com", "https://example.com"),
            ("https://[::1]:9000/SECRET", "https://[::1]:9000"),
            ("kage://install/SECRET", "kage://install"),
            ("mailto:someone@example.com?subject=SECRET", "mailto:"),
            ("javascript:alert(document.cookie)", "javascript:"),
            ("data:text/html,SECRET", "data:"),
            ("file:///Users/me/SECRET.txt", "file:"),
        ];
        for (url, logged) in cases {
            assert_eq!(url_for_log(url), logged, "{url}");
        }
        for url in ["not a url SECRET", "www.example.com/?q=SECRET", ""] {
            let logged = url_for_log(url);
            assert!(!logged.contains("SECRET"), "{logged}");
            assert!(logged.starts_with("<unparsed URL"), "{logged}");
        }
        let long = format!("https://example.com/{}?x=1", "p".repeat(500));
        assert_eq!(url_for_log(&long), "https://example.com");
    }

    /// The body of `fn <name>` in `src`, up to the next top-level item.
    fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("fn {name} not found"));
        let rest = &src[start..];
        let end = ["\nfn ", "\npub fn ", "\nunsafe fn ", "\n/// ", "\n#["]
            .iter()
            .filter_map(|m| rest[1..].find(m).map(|i| i + 1))
            .min()
            .unwrap_or(rest.len());
        &rest[..end]
    }

    /// Windows: a browser Kage starts with `ShellExecuteExW` is Kage's
    /// child and joins its `KILL_ON_JOB_CLOSE` Job Object, so quitting Kage
    /// or a crash would close it. `open_url` must hand the URL to
    /// Explorer's desktop first and open it in-process only as the
    /// fallback. The Windows code doesn't build here, so this reads it.
    #[test]
    fn windows_open_url_hands_the_url_to_the_desktop_shell_first() {
        let src = include_str!("windows/shell.rs");
        let open = fn_body(src, "open_url_impl");
        let desktop = open
            .find("open_url_via_desktop_shell(url)")
            .expect("open_url_impl must try the desktop shell");
        let fallback = open
            .find("open_url_in_process(url)")
            .expect("open_url_impl keeps the in-process fallback");
        assert!(desktop < fallback, "desktop shell first:\n{open}");
        assert!(
            !open.contains("ShellExecuteExW("),
            "open_url_impl must not call ShellExecuteExW directly:\n{open}"
        );
        // Logs carry `url_for_log` only, never the URL itself.
        assert!(open.contains("url_for_log(url)"), "{open}");
        assert!(
            !open.contains("{url}") && !open.contains(", url)"),
            "{open}"
        );

        // ShellExecute runs in Explorer, reached through the desktop window.
        let execute = fn_body(src, "desktop_shell_execute");
        for step in [
            "SWC_DESKTOP",
            "SID_STopLevelBrowser",
            "SVGIO_BACKGROUND",
            "IShellDispatch2",
            "shell.ShellExecute(",
        ] {
            assert!(execute.contains(step), "missing {step}:\n{execute}");
        }
        assert!(!execute.contains("ShellExecuteExW"), "{execute}");

        // The only in-process ShellExecuteExW is the fallback.
        assert_eq!(src.matches("ShellExecuteExW(&mut").count(), 1);
        assert!(fn_body(src, "open_url_in_process").contains("ShellExecuteExW(&mut"));
    }

    use super::{system_command, system_command_candidates};

    #[test]
    fn candidates_start_with_the_primary_program() {
        for cmd in ["terminal", "lock", "filemanager", "no-such-command"] {
            let candidates = system_command_candidates(cmd);
            assert_eq!(candidates.first(), Some(&system_command(cmd)), "{cmd}");
        }
    }

    #[test]
    fn candidates_never_repeat_a_program() {
        let candidates = system_command_candidates("terminal");
        for (i, (program, _)) in candidates.iter().enumerate() {
            assert!(
                candidates[i + 1..].iter().all(|(p, _)| p != program),
                "{program} listed twice"
            );
        }
    }

    #[test]
    fn only_the_terminal_has_alternatives() {
        for cmd in ["lock", "sleep", "filemanager", "restart", "no-such-command"] {
            assert_eq!(system_command_candidates(cmd).len(), 1, "{cmd}");
        }
    }
}

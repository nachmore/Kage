//! Pure policy for importing the login-shell environment: parsing the
//! probe's output, deciding which variables may cross over, and planning
//! the overlay for one child process. No I/O, no globals — everything is
//! unit-tested below.
//!
//! The overlay rules, in order:
//!
//! 1. **Variables set (or removed) on the `Command` itself always win.**
//!    Whatever Kage deliberately puts on a child (`PYTHONIOENCODING`,
//!    `KAGE_URL`, …) is never touched.
//! 2. **`PATH` is merged**, not replaced: login-shell entries first (the
//!    precedence the user chose in their rc files — what a terminal would
//!    see), then any inherited entries the login shell didn't list, then,
//!    at the lowest precedence, the well-known user bin directories that
//!    exist. The last group is there even when the probe worked: a probe
//!    can succeed with a *degraded* PATH (csh/tcsh `-i -c` never reads
//!    `~/.login`; a `.zshrc` that `exec`s tmux forces the `-l -c` retry,
//!    which skips `.zshrc`), and appending existing directories last can't
//!    shadow anything the user ordered.
//! 3. **Every other variable is add-missing only.** A variable already in
//!    Kage's own environment is never overridden. The probe shell is
//!    spawned *by Kage*, so it inherits Kage's environment; anything in its
//!    output that Kage lacks was added by the user's rc files. "Add
//!    missing" is therefore exactly "import what the rc files exported" —
//!    and nothing launchd set deliberately can be clobbered.
//! 4. **An explicit denylist** keeps out identity/session state that must
//!    come from launchd or the display manager (even when Kage's
//!    environment lacks it), the probe shell's own bookkeeping, terminal
//!    state that would mislead a non-terminal child (or is plain wrong in
//!    the probe: `export GPG_TTY=$(tty)` evaluates to "not a tty" there),
//!    startup-file hooks (`BASH_ENV`, `ENV`) that would make every
//!    non-interactive shell a tool runs re-run rc code, loader injection
//!    (`LD_PRELOAD`, `LD_AUDIT`), and Kage's own `KAGE_*` child-process
//!    namespace. Two contextual rules sit on top:
//!    * `SSH_AUTH_SOCK` is the one `SSH_*` variable that may cross, and
//!      only when the probe did not also export `SSH_AGENT_PID` — that pair
//!      is the signature of an rc file running `eval $(ssh-agent)`, which
//!      would point every child at a fresh, key-less agent. A socket the rc
//!      file merely points at (a gpg-agent / 1Password / systemd user
//!      agent) is what git-over-ssh in an MCP server needs on Linux
//!      sessions that start no keyring agent. Rule 3 still applies: an
//!      inherited `SSH_AUTH_SOCK` (macOS always has launchd's) wins.
//!    * **Temporary AWS credentials are not imported.** When the probe
//!      exported `AWS_SESSION_TOKEN` / `AWS_SECURITY_TOKEN`, the static
//!      key, secret, token, and expiry are all dropped: they expire within
//!      hours, Kage keeps an imported value for its whole run, and an SDK
//!      that finds expired keys in the environment uses them instead of
//!      the refreshable profile / SSO chain. Long-lived keys (no session
//!      token) still import, and `AWS_PROFILE` / `AWS_REGION` always do.
//!
//!    Deliberately **not** denied, for terminal parity: `LD_LIBRARY_PATH`,
//!    `NODE_OPTIONS`, `PYTHONPATH` and friends. They change how tools
//!    behave, but the user exported them for exactly that, and the agent
//!    should see what their terminal sees. (On macOS, `DYLD_*` can't come
//!    across at all: the payload's `/bin/sh` and `/usr/bin/env` are
//!    SIP-protected, so dyld strips `DYLD_*` before `env` prints.)
//! 5. **Size limits** keep a pathological rc file from pushing a child's
//!    environment toward `ARG_MAX` (macOS: 1 MiB for argv + envp), which
//!    would make every spawn fail with `E2BIG`.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A single variable larger than this is skipped (PATH has its own cap).
pub(super) const MAX_VALUE_BYTES: usize = 32 * 1024;
/// Budget for all imported non-PATH variables combined.
pub(super) const MAX_IMPORT_BYTES: usize = 128 * 1024;
/// Merged PATH entries beyond this are dropped (lowest precedence first).
pub(super) const MAX_PATH_ENTRIES: usize = 256;

/// Never imported, even when Kage's own environment lacks them.
const DENY_EXACT: &[&str] = &[
    // Identity and session state owned by launchd / the login session /
    // the display manager. If Kage lacks one, inventing it from a shell
    // rc is wrong.
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "MAIL",
    "GPG_AGENT_INFO",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "XDG_SEAT",
    "XDG_VTNR",
    "XDG_SESSION_ID",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_CLASS",
    "SECURITYSESSIONID",
    "COMMAND_MODE",
    "LaunchInstanceID",
    "Apple_PubSub_Socket_Render",
    "MallocNanoZone",
    "OSLogRateLimit",
    // The probe shell's own bookkeeping.
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "PS1",
    "PS2",
    "PS3",
    "PS4",
    "PROMPT_COMMAND",
    "HISTFILE",
    "HISTSIZE",
    "HISTFILESIZE",
    "SAVEHIST",
    "HISTCONTROL",
    // Exported by csh/tcsh/ksh just for starting up.
    "HOST",
    "HOSTTYPE",
    "MACHTYPE",
    "OSTYPE",
    "VENDOR",
    "GROUP",
    "A__z",
    // Terminal state. The agent and its MCP servers talk JSON-RPC over
    // pipes; telling them they're on a terminal invites colour codes and
    // TUI behaviour.
    "TERM",
    "COLORTERM",
    "COLUMNS",
    "LINES",
    "TERMINFO",
    "TMUX",
    "STY",
    "WINDOW",
    "INSIDE_EMACS",
    "VTE_VERSION",
    "WT_SESSION",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    // `export GPG_TTY=$(tty)` (the gpg docs' recommendation) evaluates to
    // "not a tty" in the probe; pinentry would then fail in every child.
    "GPG_TTY",
    "TTY",
    // Startup-file hooks: every non-interactive bash (BASH_ENV) or POSIX
    // sh (ENV) a tool starts would source rc code — the work this whole
    // module exists to do exactly once.
    "BASH_ENV",
    "ENV",
    // Loader injection into every child.
    "LD_PRELOAD",
    "LD_AUDIT",
];

/// The one `SSH_*` variable that may be imported (see the module docs).
const SSH_AUTH_SOCK: &str = "SSH_AUTH_SOCK";
/// Exported alongside `SSH_AUTH_SOCK` by `eval $(ssh-agent)`.
const SSH_AGENT_PID: &str = "SSH_AGENT_PID";

/// Their presence marks the probe's AWS credentials as temporary.
const AWS_SESSION_MARKERS: &[&str] = &["AWS_SESSION_TOKEN", "AWS_SECURITY_TOKEN"];
/// Dropped together when the credentials are temporary.
const AWS_TEMPORARY_CREDENTIALS: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_SECURITY_TOKEN",
    "AWS_CREDENTIAL_EXPIRATION",
];

/// Never imported: any name starting with one of these.
const DENY_PREFIXES: &[&str] = &[
    // Implementation-private by convention: __CF_USER_TEXT_ENCODING,
    // __CFBundleIdentifier, __MISE_DIFF, __fish_*, …
    "__",
    // launchd service identity (XPC_SERVICE_NAME, XPC_FLAGS).
    "XPC_",
    // SSH session and agent state. `SSH_AUTH_SOCK` has its own rule in
    // `build_login_env`.
    "SSH_",
    // Kage's own child-process contract: the probe marker, KAGE_URL /
    // KAGE_RESPONSE for proxy scripts, … A stale value from an rc file
    // must never leak into it.
    "KAGE_",
    // Terminal emulators / multiplexers / editors' integrated terminals.
    "TERM_",
    "TMUX_",
    "ITERM_",
    "KITTY_",
    "WEZTERM_",
    "ALACRITTY_",
    "GHOSTTY_",
    "VSCODE_",
    // Per-directory state captured from the probe's working directory.
    "DIRENV_",
    // Prompt frameworks.
    "P9K_",
    "_P9K_",
    "POWERLEVEL9K_",
    "STARSHIP_",
];

/// The resolved login-shell environment, already filtered by policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoginEnv {
    /// The login shell's PATH, deduplicated and capped.
    pub path: String,
    /// Importable non-PATH variables, sorted by name.
    pub vars: Vec<(String, String)>,
}

/// Whether a variable from the login shell may be imported at all.
/// Names must be plain identifiers — that also rules out exported bash
/// functions (`BASH_FUNC_name%%`), which are code, not configuration.
pub(super) fn is_importable(name: &str) -> bool {
    let mut chars = name.chars();
    let valid_identifier = matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
    valid_identifier
        && name != "PATH"
        && !DENY_EXACT.contains(&name)
        && !DENY_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The bytes strictly between the first `start` marker and the next `end`
/// marker after it. rc-file noise (motd banners, `echo`s, terminal title
/// escapes, `.zlogout` output) lands outside the markers and is ignored.
pub(super) fn extract_block<'a>(stdout: &'a [u8], start: &str, end: &str) -> Option<&'a [u8]> {
    let from = find(stdout, start.as_bytes())? + start.len();
    let len = find(&stdout[from..], end.as_bytes())?;
    Some(&stdout[from..from + len])
}

/// Index of the first occurrence of `needle` in `haystack`.
pub(super) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Parse `env -0` output: NUL-separated `NAME=VALUE` entries. Values may
/// contain `=` and newlines. Entries without `=`, with an empty name, or
/// that aren't UTF-8 are skipped; the first occurrence of a name wins.
pub(super) fn parse_env_block(block: &[u8]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    block
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let eq = entry.iter().position(|b| *b == b'=')?;
            let name = std::str::from_utf8(&entry[..eq]).ok()?;
            let value = std::str::from_utf8(&entry[eq + 1..]).ok()?;
            (!name.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .filter(|(name, _)| seen.insert(name.clone()))
        .collect()
}

/// Parse `/usr/bin/env` output in whichever format the payload produced:
/// NUL-separated (`env -0`) when the block contains a NUL, else the
/// newline-separated fallback the payload prints when `env -0` isn't
/// supported (macOS 11 and 12.0–12.2 ship an `env` without `-0`; it only
/// arrived with shell_cmds-240 in macOS 12.3). A real `env -0` block
/// always contains a NUL — at least the probe marker variable is set — and
/// a newline-separated one never does (values can't contain NUL).
pub(super) fn parse_env_output(block: &[u8]) -> Vec<(String, String)> {
    if block.contains(&0) {
        parse_env_block(block)
    } else {
        parse_env_lines(block)
    }
}

/// Parse newline-separated `env` output, conservatively. A new variable
/// starts at a line of the form `NAME=…` where `NAME` is a plain
/// identifier (`^[A-Za-z_][A-Za-z0-9_]*=`); any other line continues the
/// previous variable's value (a value with embedded newlines). Lines
/// before the first variable are dropped. The format is ambiguous — a
/// multi-line value whose continuation line happens to look like
/// `NAME=…` is split into two variables — which is why the NUL format is
/// preferred; the policy still filters whatever comes out.
pub(super) fn parse_env_lines(block: &[u8]) -> Vec<(String, String)> {
    let mut entries: Vec<Vec<u8>> = Vec::new();
    for line in block.split(|b| *b == b'\n') {
        if starts_with_assignment(line) {
            entries.push(line.to_vec());
        } else if let Some(last) = entries.last_mut() {
            last.push(b'\n');
            last.extend_from_slice(line);
        }
    }
    // `env` ends every entry with a newline, so the split leaves one empty
    // continuation on the last value; drop exactly that.
    if block.ends_with(b"\n") {
        if let Some(last) = entries.last_mut() {
            last.pop();
        }
    }
    let joined: Vec<u8> = entries.join(&0u8);
    parse_env_block(&joined)
}

/// `^[A-Za-z_][A-Za-z0-9_]*=`
fn starts_with_assignment(line: &[u8]) -> bool {
    let Some(eq) = line.iter().position(|b| *b == b'=') else {
        return false;
    };
    let name = &line[..eq];
    matches!(name.first(), Some(c) if *c == b'_' || c.is_ascii_alphabetic())
        && name.iter().all(|c| *c == b'_' || c.is_ascii_alphanumeric())
}

/// Apply the policy to a parsed login environment. `None` when it has no
/// usable PATH — then the probe didn't really work and we'd rather fall
/// back than import a half-environment.
pub(super) fn build_login_env(parsed: Vec<(String, String)>) -> Option<LoginEnv> {
    let path = parsed
        .iter()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| merge_paths([value.as_str()]))
        .filter(|p| !p.is_empty())?;

    let has = |wanted: &str| parsed.iter().any(|(name, _)| name == wanted);
    let ssh_sock_ok = !has(SSH_AGENT_PID);
    let temporary_aws = AWS_SESSION_MARKERS.iter().any(|m| has(m));

    let mut vars: Vec<(String, String)> = parsed
        .into_iter()
        .filter(|(name, _)| {
            if name == SSH_AUTH_SOCK {
                ssh_sock_ok
            } else if temporary_aws && AWS_TEMPORARY_CREDENTIALS.contains(&name.as_str()) {
                false
            } else {
                is_importable(name)
            }
        })
        .filter(|(_, value)| value.len() <= MAX_VALUE_BYTES)
        .collect();
    vars.sort();
    let mut budget = MAX_IMPORT_BYTES;
    vars.retain(|(name, value)| {
        let cost = name.len() + value.len() + 2;
        if cost > budget {
            return false;
        }
        budget -= cost;
        true
    });
    Some(LoginEnv { path, vars })
}

/// Merge colon-separated PATH strings in precedence order, dropping empty
/// and duplicate entries and capping the entry count.
pub(super) fn merge_paths<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut seen = HashSet::new();
    parts
        .into_iter()
        .flat_map(|p| p.split(':'))
        .filter(|entry| !entry.is_empty() && seen.insert(*entry))
        .take(MAX_PATH_ENTRIES)
        .collect::<Vec<_>>()
        .join(":")
}

/// Well-known user bin directories, appended to every merged PATH at the
/// lowest precedence: all a bare `kiro-cli` / `builder-mcp` has to go on
/// when no login environment is available (probe failed, timed out, or is
/// still running), and a safety net when the probe returned a degraded
/// PATH. The caller filters out directories that don't exist.
pub(super) fn fallback_dir_candidates(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.extend(
            ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"].map(PathBuf::from),
        );
    }
    if let Some(home) = home {
        dirs.extend(
            [".local/bin", ".toolbox/bin", ".cargo/bin", "bin"]
                .iter()
                .map(|d| home.join(d)),
        );
    }
    if cfg!(target_os = "linux") {
        dirs.extend(
            [
                "/usr/local/bin",
                "/home/linuxbrew/.linuxbrew/bin",
                "/snap/bin",
            ]
            .map(PathBuf::from),
        );
    }
    dirs
}

/// The variables to set on one child. See the module docs for the rules.
///
/// * `inherited` — Kage's own environment (`std::env::var_os`).
/// * `explicit` — whether the `Command` already sets or removes a name.
/// * `fallback_dirs` — existing well-known dirs, appended to PATH at the
///   lowest precedence.
pub(super) fn plan_overlay(
    login: Option<&LoginEnv>,
    inherited: &dyn Fn(&str) -> Option<OsString>,
    explicit: &dyn Fn(&str) -> bool,
    fallback_dirs: &[String],
) -> Vec<(String, String)> {
    let mut out = Vec::new();

    if !explicit("PATH") {
        // A non-UTF-8 inherited PATH can't be merged without corrupting
        // it; leave it alone rather than guess.
        let current = match inherited("PATH") {
            None => Some(String::new()),
            Some(os) => os.into_string().ok(),
        };
        if let Some(current) = current {
            let fallback = fallback_dirs.join(":");
            let login_path = login.map(|env| env.path.as_str()).unwrap_or("");
            let merged = merge_paths([login_path, current.as_str(), fallback.as_str()]);
            if !merged.is_empty() && merged != current {
                out.push(("PATH".to_string(), merged));
            }
        }
    }

    if let Some(env) = login {
        out.extend(
            env.vars
                .iter()
                .filter(|(name, _)| !explicit(name) && inherited(name).is_none())
                .cloned(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, OsString> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect()
    }

    fn login(path: &str, vars: &[(&str, &str)]) -> LoginEnv {
        LoginEnv {
            path: path.to_string(),
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn plan(
        login: Option<&LoginEnv>,
        inherited: &HashMap<String, OsString>,
        explicit: &[&str],
        fallback: &[&str],
    ) -> HashMap<String, String> {
        let fallback: Vec<String> = fallback.iter().map(|s| s.to_string()).collect();
        plan_overlay(
            login,
            &|k| inherited.get(k).cloned(),
            &|k| explicit.contains(&k),
            &fallback,
        )
        .into_iter()
        .collect()
    }

    const LAUNCHD_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

    #[test]
    fn merge_keeps_precedence_and_dedupes() {
        assert_eq!(
            merge_paths(["/u/.toolbox/bin:/opt/homebrew/bin:/usr/bin", LAUNCHD_PATH]),
            "/u/.toolbox/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }

    #[test]
    fn merge_drops_empty_entries() {
        assert_eq!(merge_paths(["::/a::/b:", ":/b:/c:"]), "/a:/b:/c");
        assert_eq!(merge_paths(["", ""]), "");
    }

    #[test]
    fn merge_caps_huge_paths_keeping_highest_precedence() {
        let huge: Vec<String> = (0..MAX_PATH_ENTRIES + 50)
            .map(|i| format!("/d{i}"))
            .collect();
        // Rc files that append on every nested shell produce duplicates
        // too; those collapse before the cap applies.
        let joined = format!("{0}:{0}", huge.join(":"));
        let merged = merge_paths([joined.as_str()]);
        let entries: Vec<&str> = merged.split(':').collect();
        assert_eq!(entries.len(), MAX_PATH_ENTRIES);
        assert_eq!(entries[0], "/d0");
        assert_eq!(
            entries[MAX_PATH_ENTRIES - 1],
            format!("/d{}", MAX_PATH_ENTRIES - 1)
        );
    }

    #[test]
    fn extract_ignores_rc_noise_around_markers() {
        let out = b"Welcome!\n\x1b]0;title\x07STARTPATH=/x\0ENDbye from .zlogout\n";
        assert_eq!(extract_block(out, "START", "END"), Some(&b"PATH=/x\0"[..]));
    }

    #[test]
    fn extract_requires_both_markers_in_order() {
        assert_eq!(extract_block(b"no markers", "START", "END"), None);
        assert_eq!(extract_block(b"START only", "START", "END"), None);
        assert_eq!(extract_block(b"END before START", "START", "END"), None);
        assert_eq!(extract_block(b"STARTEND", "START", "END"), Some(&b""[..]));
    }

    #[test]
    fn parse_handles_equals_newlines_and_junk() {
        let block =
            b"A=1\0B=x=y\0C=line1\nline2\0no_equals\0=empty_name\0D=\0A=dup\0\xff\xfe=bad\0";
        let parsed = parse_env_block(block);
        assert_eq!(
            parsed,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "x=y".to_string()),
                ("C".to_string(), "line1\nline2".to_string()),
                ("D".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn importable_allows_typical_tool_config() {
        for name in [
            "JAVA_HOME",
            "GOPATH",
            "NVM_DIR",
            "PYENV_ROOT",
            "HTTPS_PROXY",
            "https_proxy",
            "NO_PROXY",
            "SSL_CERT_FILE",
            "NODE_EXTRA_CA_CERTS",
            "AWS_PROFILE",
            "AWS_REGION",
            "LANG",
            "LC_ALL",
            "HOMEBREW_PREFIX",
            // Kept for terminal parity: the user exported them on purpose.
            "LD_LIBRARY_PATH",
            "NODE_OPTIONS",
            "PYTHONPATH",
        ] {
            assert!(is_importable(name), "{name} should be importable");
        }
    }

    #[test]
    fn importable_rejects_session_terminal_and_kage_state() {
        for name in [
            "PATH",
            "HOME",
            "USER",
            "TMPDIR",
            "SHELL",
            "SSH_AUTH_SOCK",
            "SSH_AGENT_PID",
            "__CF_USER_TEXT_ENCODING",
            "__CFBundleIdentifier",
            "XPC_SERVICE_NAME",
            "XPC_FLAGS",
            "KAGE_RESOLVING_ENVIRONMENT",
            "KAGE_URL",
            "PWD",
            "SHLVL",
            "_",
            "TERM",
            "TERM_PROGRAM",
            "TMUX",
            "ITERM_SESSION_ID",
            "DIRENV_DIFF",
            "BASH_FUNC_module%%",
            "GPG_TTY",
            "TTY",
            "BASH_ENV",
            "ENV",
            "LD_PRELOAD",
            "LD_AUDIT",
            "SSH_CONNECTION",
            "SSH_TTY",
            "1NUMERIC",
            "has-dash",
            "",
        ] {
            assert!(!is_importable(name), "{name:?} must not be importable");
        }
    }

    #[test]
    fn build_requires_a_path() {
        assert_eq!(
            build_login_env(vec![("JAVA_HOME".into(), "/j".into())]),
            None
        );
        assert_eq!(build_login_env(vec![("PATH".into(), "::".into())]), None);
    }

    #[test]
    fn build_filters_denied_and_oversized_and_sorts() {
        let env = build_login_env(vec![
            ("PATH".into(), "/a:/a:/b".into()),
            ("ZED".into(), "z".into()),
            ("HOME".into(), "/elsewhere".into()),
            ("HUGE".into(), "x".repeat(MAX_VALUE_BYTES + 1)),
            ("JAVA_HOME".into(), "/j".into()),
        ])
        .expect("has PATH");
        assert_eq!(env.path, "/a:/b");
        assert_eq!(
            env.vars,
            vec![
                ("JAVA_HOME".to_string(), "/j".to_string()),
                ("ZED".to_string(), "z".to_string())
            ]
        );
    }

    #[test]
    fn build_enforces_total_budget() {
        let big = "v".repeat(MAX_VALUE_BYTES - 100);
        let parsed: Vec<(String, String)> = std::iter::once(("PATH".to_string(), "/a".to_string()))
            .chain((0..10).map(|i| (format!("BIG_{i}"), big.clone())))
            .collect();
        let env = build_login_env(parsed).expect("has PATH");
        let total: usize = env.vars.iter().map(|(k, v)| k.len() + v.len() + 2).sum();
        assert!(total <= MAX_IMPORT_BYTES, "{total}");
        assert!(!env.vars.is_empty() && env.vars.len() < 10);
    }

    #[test]
    fn overlay_from_launchd_env_imports_rc_additions() {
        // What Finder/Dock hands a GUI app.
        let inherited = env_of(&[
            ("PATH", LAUNCHD_PATH),
            ("HOME", "/Users/me"),
            ("TMPDIR", "/var/folders/xy/T/"),
            ("SSH_AUTH_SOCK", "/private/tmp/launchd/Listeners"),
        ]);
        let login = login(
            "/Users/me/.toolbox/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            &[("JAVA_HOME", "/j"), ("HTTPS_PROXY", "http://proxy:3128")],
        );
        let planned = plan(Some(&login), &inherited, &[], &["/Users/me/.toolbox/bin"]);
        assert_eq!(
            planned["PATH"],
            "/Users/me/.toolbox/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "a fallback dir the login PATH already has isn't repeated"
        );
        assert_eq!(planned["JAVA_HOME"], "/j");
        assert_eq!(planned["HTTPS_PROXY"], "http://proxy:3128");
        assert_eq!(planned.len(), 3);
    }

    #[test]
    fn overlay_never_overrides_inherited_values() {
        let inherited = env_of(&[("PATH", "/usr/bin"), ("JAVA_HOME", "/launchd/java")]);
        let login = login("/usr/bin", &[("JAVA_HOME", "/rc/java"), ("GOPATH", "/go")]);
        let planned = plan(Some(&login), &inherited, &[], &[]);
        assert!(!planned.contains_key("JAVA_HOME"));
        assert!(!planned.contains_key("PATH"), "unchanged PATH isn't re-set");
        assert_eq!(planned["GOPATH"], "/go");
    }

    #[test]
    fn overlay_respects_values_set_on_the_command() {
        let inherited = env_of(&[("PATH", LAUNCHD_PATH)]);
        let login = login(
            "/x/bin:/usr/bin",
            &[("PYTHONIOENCODING", "latin-1"), ("A", "1")],
        );
        let planned = plan(Some(&login), &inherited, &["PATH", "PYTHONIOENCODING"], &[]);
        assert!(!planned.contains_key("PATH"));
        assert!(!planned.contains_key("PYTHONIOENCODING"));
        assert_eq!(planned["A"], "1");
    }

    #[test]
    fn overlay_inherited_extras_follow_login_entries() {
        let inherited = env_of(&[("PATH", "/venv/bin:/usr/bin")]);
        let login = login("/opt/homebrew/bin:/usr/bin", &[]);
        let planned = plan(Some(&login), &inherited, &[], &[]);
        assert_eq!(planned["PATH"], "/opt/homebrew/bin:/usr/bin:/venv/bin");
    }

    #[test]
    fn overlay_without_login_env_appends_fallback_dirs() {
        let inherited = env_of(&[("PATH", LAUNCHD_PATH)]);
        let planned = plan(
            None,
            &inherited,
            &[],
            &["/Users/me/.toolbox/bin", "/usr/bin"],
        );
        assert_eq!(
            planned["PATH"],
            format!("{LAUNCHD_PATH}:/Users/me/.toolbox/bin")
        );
        assert_eq!(planned.len(), 1);
    }

    #[test]
    fn overlay_appends_fallback_dirs_to_a_degraded_login_path() {
        // csh/tcsh `-i -c` never reads ~/.login, and a `.zshrc` that
        // `exec`s tmux forces the `-l -c` retry, which skips `.zshrc`:
        // the probe "works" but its PATH lacks the user's tool dirs.
        let inherited = env_of(&[("PATH", LAUNCHD_PATH)]);
        for (degraded, appended) in [
            (
                "/usr/bin:/bin",
                &[
                    "/Users/me/.toolbox/bin",
                    "/opt/homebrew/bin",
                    "/Users/me/.local/bin",
                ][..],
            ),
            (
                "/opt/homebrew/bin:/usr/bin:/bin",
                &["/Users/me/.toolbox/bin", "/Users/me/.local/bin"][..],
            ),
        ] {
            let login = login(degraded, &[("LANG", "en_US.UTF-8")]);
            let planned = plan(
                Some(&login),
                &inherited,
                &[],
                &[
                    "/Users/me/.toolbox/bin",
                    "/opt/homebrew/bin",
                    "/Users/me/.local/bin",
                ],
            );
            let entries: Vec<&str> = planned["PATH"].split(':').collect();
            assert_eq!(
                &entries[..degraded.split(':').count()],
                degraded.split(':').collect::<Vec<_>>().as_slice(),
                "login entries keep the highest precedence"
            );
            assert_eq!(
                &entries[entries.len() - appended.len()..],
                appended,
                "missing user bin dirs are appended last, after inherited extras"
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|e| **e == "/opt/homebrew/bin")
                    .count(),
                1
            );
            assert_eq!(planned["LANG"], "en_US.UTF-8");
        }
    }

    #[test]
    fn overlay_fallback_dirs_follow_inherited_extras() {
        let inherited = env_of(&[("PATH", "/venv/bin:/usr/bin")]);
        let login = login("/usr/bin:/bin", &[]);
        let planned = plan(Some(&login), &inherited, &[], &["/u/.cargo/bin"]);
        assert_eq!(planned["PATH"], "/usr/bin:/bin:/venv/bin:/u/.cargo/bin");
    }

    #[test]
    fn overlay_with_no_inherited_path_still_sets_one() {
        let planned = plan(None, &env_of(&[]), &[], &["/a"]);
        assert_eq!(planned["PATH"], "/a");
        assert!(plan(None, &env_of(&[]), &[], &[]).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn overlay_leaves_non_utf8_inherited_path_alone() {
        use std::os::unix::ffi::OsStringExt;
        let mut inherited = HashMap::new();
        inherited.insert(
            "PATH".to_string(),
            OsString::from_vec(b"/usr/bin:/\xff/bin".to_vec()),
        );
        let login = login("/x/bin", &[("A", "1")]);
        let planned = plan(Some(&login), &inherited, &[], &[]);
        assert!(!planned.contains_key("PATH"));
        assert_eq!(planned["A"], "1");
    }

    fn names(env: &LoginEnv) -> Vec<&str> {
        env.vars.iter().map(|(k, _)| k.as_str()).collect()
    }

    fn build(pairs: &[(&str, &str)]) -> LoginEnv {
        build_login_env(
            std::iter::once(("PATH".to_string(), "/usr/bin".to_string()))
                .chain(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())))
                .collect(),
        )
        .expect("has PATH")
    }

    #[test]
    fn ssh_auth_sock_imports_only_without_an_rc_started_agent() {
        // An rc file pointing at an existing agent socket (gpg-agent,
        // 1Password, a systemd user agent): import it.
        let env = build(&[
            ("SSH_AUTH_SOCK", "/run/user/1000/gnupg/S.gpg-agent.ssh"),
            ("SSH_CONNECTION", "10.0.0.1 22 10.0.0.2 22"),
        ]);
        assert_eq!(names(&env), ["SSH_AUTH_SOCK"], "other SSH_* stay denied");

        // `eval $(ssh-agent)`: a fresh, key-less agent per probe. Drop it.
        let env = build(&[
            ("SSH_AUTH_SOCK", "/tmp/ssh-XXXX/agent.123"),
            ("SSH_AGENT_PID", "124"),
        ]);
        assert!(names(&env).is_empty(), "{:?}", env.vars);
    }

    #[test]
    fn ssh_auth_sock_never_overrides_the_inherited_one() {
        let login = build(&[("SSH_AUTH_SOCK", "/rc/agent.sock")]);
        // macOS: launchd always provides one.
        let inherited = env_of(&[
            ("PATH", "/usr/bin"),
            (
                "SSH_AUTH_SOCK",
                "/private/tmp/com.apple.launchd.x/Listeners",
            ),
        ]);
        assert!(!plan(Some(&login), &inherited, &[], &[]).contains_key("SSH_AUTH_SOCK"));
        // A Linux session without a keyring agent: import.
        let inherited = env_of(&[("PATH", "/usr/bin")]);
        assert_eq!(
            plan(Some(&login), &inherited, &[], &[])["SSH_AUTH_SOCK"],
            "/rc/agent.sock"
        );
    }

    #[test]
    fn temporary_aws_credentials_are_not_imported() {
        for marker in ["AWS_SESSION_TOKEN", "AWS_SECURITY_TOKEN"] {
            let env = build(&[
                ("AWS_ACCESS_KEY_ID", "ASIAEXAMPLE"),
                ("AWS_SECRET_ACCESS_KEY", "secret"),
                (marker, "token"),
                ("AWS_CREDENTIAL_EXPIRATION", "2026-11-03T20:00:00Z"),
                ("AWS_PROFILE", "dev"),
                ("AWS_REGION", "us-west-2"),
                ("AWS_DEFAULT_REGION", "us-west-2"),
            ]);
            assert_eq!(
                names(&env),
                ["AWS_DEFAULT_REGION", "AWS_PROFILE", "AWS_REGION"],
                "with {marker}"
            );
        }
    }

    #[test]
    fn long_lived_aws_keys_still_import() {
        let env = build(&[
            ("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_PROFILE", "dev"),
        ]);
        assert_eq!(
            names(&env),
            ["AWS_ACCESS_KEY_ID", "AWS_PROFILE", "AWS_SECRET_ACCESS_KEY"]
        );
    }

    #[test]
    fn env_lines_parse_conservatively() {
        let out = b"PATH=/a:/b\nMULTI=line1\nline two = not a var\n  indented=x\nlast\n\
                    _UNDER=1\nA1=x=y\nEMPTY=\n";
        assert_eq!(
            parse_env_lines(out),
            vec![
                ("PATH".to_string(), "/a:/b".to_string()),
                (
                    "MULTI".to_string(),
                    "line1\nline two = not a var\n  indented=x\nlast".to_string()
                ),
                ("_UNDER".to_string(), "1".to_string()),
                ("A1".to_string(), "x=y".to_string()),
                ("EMPTY".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn env_lines_drop_leading_junk_and_keep_trailing_newlines_in_values() {
        // Lines before the first `NAME=` have no variable to belong to.
        // A value that itself ends in a newline prints as an empty line
        // before the next variable.
        let out = b"env: illegal option -- 0\n=nameless\nA=1\n\nB=2";
        assert_eq!(
            parse_env_lines(out),
            vec![
                ("A".to_string(), "1\n".to_string()),
                ("B".to_string(), "2".to_string()),
            ]
        );
        assert!(parse_env_lines(b"").is_empty());
        assert!(parse_env_lines(b"\n\n").is_empty());
        // Names that aren't identifiers never start a variable.
        assert_eq!(
            parse_env_lines(b"A=1\n1BAD=2\nhas-dash=3\n"),
            vec![("A".to_string(), "1\n1BAD=2\nhas-dash=3".to_string())]
        );
    }

    #[test]
    fn env_output_format_is_detected_by_nul() {
        assert_eq!(
            parse_env_output(b"A=1\nstill A\0B=2\0"),
            vec![
                ("A".to_string(), "1\nstill A".to_string()),
                ("B".to_string(), "2".to_string()),
            ]
        );
        assert_eq!(
            parse_env_output(b"A=1\nB=2\n"),
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string()),
            ]
        );
        assert!(parse_env_output(b"").is_empty());
    }

    #[test]
    fn fallback_candidates_include_user_tool_dirs() {
        let dirs = fallback_dir_candidates(Some(Path::new("/home/me")));
        assert!(dirs.contains(&PathBuf::from("/home/me/.toolbox/bin")));
        assert!(dirs.contains(&PathBuf::from("/home/me/.local/bin")));
        assert!(fallback_dir_candidates(None)
            .iter()
            .all(|d| !d.starts_with("/home/me")));
    }
}

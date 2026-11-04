//! Runs the user's login shell once to capture its environment.
//!
//! Shared by macOS and Linux (both POSIX; the code is identical), so it
//! lives here rather than being duplicated under `os/macos` + `os/linux`.
//!
//! Robustness against hostile-ish rc files:
//! * **Noise** (banners, `echo`, title escapes, `.zlogout` output): the
//!   environment is bracketed by per-run nonce markers and everything
//!   outside them is ignored.
//! * **Prompts** (`read`, compinit's "insecure directories?"): stdin is
//!   `/dev/null`, so reads hit EOF immediately.
//! * **No controlling terminal**: the shell runs in its own session
//!   (`setsid`), so nothing can grab or wait on Kage's terminal in dev
//!   mode (no SIGTTIN/SIGTTOU stops), and `exec tmux` fails fast with
//!   "not a terminal" instead of hanging.
//! * **Hangs**: a hard deadline; on expiry (and on every other forced
//!   kill: output overflow, a shell that won't exit after printing) every
//!   process in the probe's *session* is SIGKILLed, not just its process
//!   group. Without job control, background jobs an rc file starts stay in
//!   the shell's group; but an rc file can turn job control on (`set -m`
//!   in bash/sh, `setopt monitor` in zsh), and then every job gets a
//!   process group of its own that `killpg` never reaches. They do all stay in the
//!   shell's session (`setsid` made the shell its leader, so the session
//!   id is the shell's pid), so after `killpg` we sweep the session:
//!   Linux reads field 6 of `/proc/<pid>/stat`; macOS lists every pid
//!   (`proc_listallpids`) and asks `getsid` for each — which works across
//!   sessions there, root's processes included. Only a process that
//!   started its own session (a daemon calling `setsid`, e.g.
//!   `eval $(ssh-agent)`) escapes, as it would a closed terminal.
//! * **Lingering background jobs holding stdout**: we stop reading as soon
//!   as the end marker arrives, or shortly after the shell exits, rather
//!   than waiting for EOF.
//! * **Endless output**: capped at [`MAX_OUTPUT`]; the shell is killed.
//! * **`exec` in an interactive rc** (`exec zsh`, unguarded `exec tmux`):
//!   no markers come back, so we retry once as a non-interactive login
//!   shell, which skips `.zshrc` / `.bashrc`.
//! * **No `env -0`** (macOS 11 and 12.0–12.2 — the `-0` flag reached
//!   Apple's `env` with shell_cmds-240 in macOS 12.3; `minimumSystemVersion`
//!   is 11.0): the payload falls back to plain `/usr/bin/env` inside the
//!   same shell run, and [`policy::parse_env_output`] tells the formats
//!   apart (a NUL-separated block always contains a NUL).
//!
//! The probe sets `KAGE_RESOLVING_ENVIRONMENT=1` so rc files can skip
//! work that only makes sense in a real terminal (or that has side
//! effects: credential refreshes, browser launches, network calls).

use super::policy::{self, LoginEnv};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Hard cap for the whole probe (all attempts). Matches VS Code's default
/// `application.shellEnvironmentResolutionTimeout`. Healthy rc files —
/// even oh-my-zsh + nvm + conda — finish in well under two seconds.
pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Set on the probe shell so rc files can detect it.
pub(super) const MARKER_ENV: &str = "KAGE_RESOLVING_ENVIRONMENT";

/// Stop collecting stdout past this; the rest is junk.
const MAX_OUTPUT: usize = 4 * 1024 * 1024;

/// After the end marker (or the shell's exit) is seen, how long to give
/// the shell to exit (or a straggling write to land) before moving on.
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// How the login shell spells "interactive login, run this command".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShellKind {
    /// sh, bash, zsh, dash, ksh, fish, … — `-i -l -c`.
    Posix,
    /// csh/tcsh: `-l` is only valid as the sole argument; `.cshrc` is read
    /// by every csh anyway, so `-i -c` it is.
    Csh,
    /// nushell: same flags, but externals need the `^` sigil.
    Nu,
    /// PowerShell: `-Login` must come first; `&` invokes a path.
    Pwsh,
}

pub(super) fn shell_kind(shell: &Path) -> ShellKind {
    match shell.file_name().and_then(|n| n.to_str()).unwrap_or("") {
        "csh" | "tcsh" => ShellKind::Csh,
        "nu" | "nushell" => ShellKind::Nu,
        "pwsh" | "pwsh-preview" | "powershell" => ShellKind::Pwsh,
        _ => ShellKind::Posix,
    }
}

/// Argument vectors to try, in order. The payload is one external command
/// — `/bin/sh` printing the markers around `/usr/bin/env -0` — so the
/// only thing each login shell has to parse is a single-quoted string with
/// no embedded quotes (and no `!`, for csh), which every supported shell
/// treats literally. `env` dumps exactly the *exported* variables, which
/// is what a child process would inherit; `-0` keeps values with newlines
/// intact. An `env` without `-0` prints its usage to stderr and exits
/// non-zero before writing anything, so `||` reruns it newline-separated.
pub(super) fn probe_attempts(kind: ShellKind, start: &str, end: &str) -> Vec<Vec<String>> {
    let payload = format!(
        "/bin/sh -c 'printf %s $0; /usr/bin/env -0 2>/dev/null || /usr/bin/env; printf %s $1' {start} {end}"
    );
    let args = |flags: &[&str], cmd: String| -> Vec<String> {
        flags
            .iter()
            .map(|f| f.to_string())
            .chain(std::iter::once(cmd))
            .collect()
    };
    match kind {
        ShellKind::Posix => vec![
            args(&["-i", "-l", "-c"], payload.clone()),
            args(&["-l", "-c"], payload),
        ],
        ShellKind::Nu => vec![
            args(&["-i", "-l", "-c"], format!("^{payload}")),
            args(&["-l", "-c"], format!("^{payload}")),
        ],
        ShellKind::Csh => vec![args(&["-i", "-c"], payload)],
        ShellKind::Pwsh => vec![args(
            &["-Login", "-Command"],
            format!("& {}", payload.replacen("/bin/sh", "'/bin/sh'", 1)),
        )],
    }
}

/// An executable regular file that isn't a "no login" placeholder.
pub(super) fn is_usable_shell(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if !path.is_absolute() {
        return false;
    }
    if matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some("false" | "nologin")
    ) {
        return false;
    }
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// `$SHELL`, else the shell in the user's passwd/directory record (what
/// Terminal.app uses), else `/bin/sh`.
pub(super) fn select_shell() -> Option<PathBuf> {
    let from_env = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let from_passwd = || {
        nix::unistd::User::from_uid(nix::unistd::Uid::current())
            .ok()
            .flatten()
            .map(|u| u.shell)
    };
    from_env
        .filter(|p| is_usable_shell(p))
        .or_else(|| from_passwd().filter(|p| is_usable_shell(p)))
        .or_else(|| Some(PathBuf::from("/bin/sh")).filter(|p| is_usable_shell(p)))
}

/// Why resolution failed — only ever logged.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ProbeError {
    Spawn(String),
    TimedOut,
    TooMuchOutput,
    NoEnvironment,
}

impl ProbeError {
    /// Whether the probe had to be killed.
    pub(super) fn killed(&self) -> bool {
        matches!(self, Self::TimedOut | Self::TooMuchOutput)
    }
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "could not start the shell: {e}"),
            Self::TimedOut => write!(f, "timed out"),
            Self::TooMuchOutput => write!(
                f,
                "printed more than {} MiB without finishing",
                MAX_OUTPUT / (1024 * 1024)
            ),
            Self::NoEnvironment => write!(f, "the shell printed no environment"),
        }
    }
}

/// A resolved environment plus how it was obtained (for the log line).
pub(super) struct Resolved {
    pub env: LoginEnv,
    pub flags: String,
    /// The shell printed its environment but then didn't exit within
    /// [`EXIT_GRACE`] (a hanging `.zlogout`, …) and was killed.
    pub killed: bool,
}

/// Probe `shell` with an overall `timeout`.
pub(super) fn resolve_with(shell: &Path, timeout: Duration) -> Result<Resolved, ProbeError> {
    let nonce = format!(
        "{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let start = format!("__KAGE_ENV_START_{nonce}__");
    let end = format!("__KAGE_ENV_END_{nonce}__");
    let deadline = Instant::now() + timeout;
    // Only an existing directory can be a cwd (an isolated test home may
    // not exist; then the probe inherits Kage's cwd).
    let home = crate::paths::home_dir().filter(|h| h.is_dir());
    let probe_started = std::time::SystemTime::now();
    let socket_predates_probe = |sock: &str| {
        std::fs::metadata(sock)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < probe_started)
    };

    for args in probe_attempts(shell_kind(shell), &start, &end) {
        let (stdout, killed) = run_probe(shell, &args, home.as_deref(), &end, deadline)?;
        if let Some(block) = policy::extract_block(&stdout, &start, &end) {
            let env = policy::build_login_env_with(
                policy::parse_env_output(block),
                &socket_predates_probe,
            )
            .ok_or(ProbeError::NoEnvironment)?;
            let flags = args[..args.len() - 1].join(" ");
            return Ok(Resolved { env, flags, killed });
        }
        // No markers: probably an `exec` in an interactive rc. Retry with
        // the next (less interactive) form, if any.
    }
    Err(ProbeError::NoEnvironment)
}

/// Spawn one probe and collect its stdout until the end marker, EOF, the
/// shell's exit (+ grace), the output cap, or `deadline`. The child is
/// always reaped; on the failure paths everything in its session is
/// killed. Returns the output and whether the shell had to be killed.
fn run_probe(
    shell: &Path,
    args: &[String],
    cwd: Option<&Path>,
    end_marker: &str,
    deadline: Instant,
) -> Result<(Vec<u8>, bool), ProbeError> {
    let mut cmd = Command::new(shell);
    cmd.args(args)
        .env(MARKER_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(dir) = cwd {
        // Like opening a terminal: rc files that look at `$PWD`
        // (direnv, mise, asdf) see the home directory, not launchd's `/`.
        cmd.current_dir(dir);
    }
    // SAFETY: setsid is async-signal-safe. It makes the shell a session
    // and group leader (sid == pgid == pid, so `killpg` + the session
    // sweep reach everything it started) with no controlling terminal.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    // ETXTBSY (Linux): the shell binary was just written — a package
    // manager mid-upgrade, or a test fixture racing another thread's fork.
    // It clears within milliseconds.
    let mut attempt = 0;
    let mut child = loop {
        match cmd.spawn() {
            Ok(child) => break child,
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(ProbeError::Spawn(e.to_string())),
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        kill_and_reap(&mut child);
        return Err(ProbeError::Spawn("no stdout pipe".into()));
    };

    let fd = stdout.as_raw_fd();
    let end = end_marker.as_bytes();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    // Set once `try_wait` has reaped the shell: from then on its pid may
    // be reused, so it must never be signalled.
    let mut exited_at: Option<Instant> = None;

    enum Stop {
        Complete,
        Exited,
        Overflow,
        TimedOut,
    }
    let stop = loop {
        let now = Instant::now();
        if now >= deadline {
            break Stop::TimedOut;
        }
        if let Some(at) = exited_at {
            if now.duration_since(at) >= EXIT_GRACE {
                // The shell is gone but something it left behind still
                // holds the pipe open. Don't wait for it.
                break Stop::Exited;
            }
        } else if let Ok(Some(_)) = child.try_wait() {
            exited_at = Some(now);
        }

        let slice = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(50));
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, fd owned by `stdout` for the call.
        let ready = unsafe { libc::poll(&mut pfd, 1, slice.as_millis() as libc::c_int) };
        if ready <= 0 {
            // Timeout slice or EINTR — loop re-checks deadline and exit.
            continue;
        }
        let scan_from = buf.len().saturating_sub(end.len());
        match stdout.read(&mut chunk) {
            Ok(0) => break Stop::Exited, // EOF: every writer is gone.
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break Stop::Exited,
        }
        if policy::find(&buf[scan_from..], end).is_some() {
            break Stop::Complete;
        }
        if buf.len() > MAX_OUTPUT {
            break Stop::Overflow;
        }
    };
    // Closing our end makes any straggling writer get EPIPE/SIGPIPE.
    drop(stdout);

    let mut killed = false;
    if exited_at.is_none() {
        match stop {
            Stop::Complete | Stop::Exited => {
                // We have what we need; let the shell finish (`.zlogout`)
                // briefly, then make sure it's gone.
                let grace = (Instant::now() + EXIT_GRACE).min(deadline);
                while Instant::now() < grace {
                    if let Ok(Some(_)) = child.try_wait() {
                        exited_at = Some(Instant::now());
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                if exited_at.is_none() {
                    kill_and_reap(&mut child);
                    killed = true;
                }
            }
            Stop::Overflow | Stop::TimedOut => {
                kill_and_reap(&mut child);
                killed = true;
            }
        }
    }

    match stop {
        Stop::TimedOut => Err(ProbeError::TimedOut),
        Stop::Overflow => Err(ProbeError::TooMuchOutput),
        Stop::Complete | Stop::Exited => Ok((buf, killed)),
    }
}

/// SIGKILL the child's process group and then everything else in its
/// session, then reap the child. Only valid while the child is unreaped:
/// until then its pid — and so the group and session ids, which equal it
/// — can't be recycled, so nothing outside the probe can match.
fn kill_and_reap(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        if pid > 1 {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
            kill_session(pid);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// SIGKILL every process in session `sid` except Kage itself. A few
/// passes, because a process can fork between the listing and its kill;
/// a SIGKILLed one can't fork again, so the next pass catches the child.
fn kill_session(sid: libc::pid_t) {
    if sid <= 1 {
        return;
    }
    let me = std::process::id() as libc::pid_t;
    for pass in 0..4 {
        let members: Vec<libc::pid_t> = session_members(sid)
            .into_iter()
            .filter(|pid| *pid > 1 && *pid != me)
            .collect();
        if members.is_empty() {
            return;
        }
        for pid in members {
            // SAFETY: plain syscall. A member that exited since the
            // listing yields ESRCH; its pid being reused by an unrelated
            // process within these microseconds would take a full wrap of
            // the (sequentially allocated) pid space.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        if pass < 3 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Live (non-zombie) processes whose session id is `sid`.
#[cfg(target_os = "linux")]
fn session_members(sid: libc::pid_t) -> Vec<libc::pid_t> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<libc::pid_t>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| stat_session(&stat))
                .is_some_and(|(state, session)| session == sid && state != 'Z' && state != 'X')
        })
        .collect()
}

/// `(state, session)` from a `/proc/<pid>/stat` line. The command name
/// (field 2) is parenthesised and may itself contain spaces and `)`, so
/// fields are counted from the *last* `)`: state, ppid, pgrp, session.
#[cfg(any(target_os = "linux", test))]
fn stat_session(stat: &str) -> Option<(char, libc::pid_t)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let session = fields.nth(2)?.parse().ok()?;
    Some((state, session))
}

/// Processes whose session id is `sid`. `getsid` answers for processes in
/// other sessions on macOS (verified against launchd and root daemons).
#[cfg(target_os = "macos")]
fn session_members(sid: libc::pid_t) -> Vec<libc::pid_t> {
    // SAFETY: a null buffer asks for the current pid count.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    // Headroom for processes started since the count.
    let capacity = count as usize + 256;
    let mut pids: Vec<libc::pid_t> = vec![0; capacity];
    let bytes = capacity * std::mem::size_of::<libc::pid_t>();
    // SAFETY: the buffer holds `capacity` pids, `bytes` long.
    let filled = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            libc::c_int::try_from(bytes).unwrap_or(libc::c_int::MAX),
        )
    };
    pids.truncate(usize::try_from(filled).unwrap_or(0).min(capacity));
    // SAFETY: getsid is a plain query; -1 (ESRCH, …) never equals sid.
    pids.retain(|pid| *pid > 0 && unsafe { libc::getsid(*pid) } == sid);
    pids
}

/// Other Unixes have no portable process listing; `killpg` has to do.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn session_members(_sid: libc::pid_t) -> Vec<libc::pid_t> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Probe timeout for tests where hitting it *is* the failure.
    const NEVER: Duration = Duration::from_secs(60);
    /// "Returned without waiting for the timeout" — loose on purpose so a
    /// loaded CI runner can't flake it; a regression costs the full 60 s.
    const WELL_BEFORE: Duration = Duration::from_secs(20);

    /// Write an executable `/bin/sh` script standing in for a login shell.
    /// The real payload arrives as its last argument.
    fn fake_shell(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\nfor last in \"$@\"; do :; done\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn var<'a>(env: &'a LoginEnv, name: &str) -> Option<&'a str> {
        env.vars
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn kinds_by_shell_name() {
        assert_eq!(shell_kind(Path::new("/bin/zsh")), ShellKind::Posix);
        assert_eq!(
            shell_kind(Path::new("/opt/homebrew/bin/fish")),
            ShellKind::Posix
        );
        assert_eq!(shell_kind(Path::new("/bin/tcsh")), ShellKind::Csh);
        assert_eq!(shell_kind(Path::new("/opt/homebrew/bin/nu")), ShellKind::Nu);
        assert_eq!(
            shell_kind(Path::new("/usr/local/bin/pwsh")),
            ShellKind::Pwsh
        );
    }

    #[test]
    fn attempt_shapes_per_shell() {
        let posix = probe_attempts(ShellKind::Posix, "S", "E");
        assert_eq!(posix.len(), 2);
        assert_eq!(&posix[0][..3], ["-i", "-l", "-c"]);
        assert_eq!(&posix[1][..2], ["-l", "-c"]);
        assert!(posix[0][3].starts_with("/bin/sh -c '") && posix[0][3].ends_with("' S E"));

        let csh = probe_attempts(ShellKind::Csh, "S", "E");
        assert_eq!(csh.len(), 1);
        assert_eq!(&csh[0][..2], ["-i", "-c"]);

        let nu = probe_attempts(ShellKind::Nu, "S", "E");
        assert!(nu[0][3].starts_with("^/bin/sh "));

        let pwsh = probe_attempts(ShellKind::Pwsh, "S", "E");
        assert_eq!(&pwsh[0][..2], ["-Login", "-Command"]);
        assert!(pwsh[0][2].starts_with("& '/bin/sh' -c '"));
    }

    #[test]
    fn usable_shell_checks() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_usable_shell(Path::new("/bin/sh")));
        assert!(!is_usable_shell(Path::new("sh")), "relative");
        assert!(!is_usable_shell(&dir.path().join("missing")));
        assert!(!is_usable_shell(dir.path()), "directory");
        let plain = dir.path().join("not-exec");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_usable_shell(&plain));
        let nologin = fake_shell(dir.path(), "nologin", "exit 1");
        assert!(!is_usable_shell(&nologin));
    }

    #[test]
    fn noisy_rc_and_prompts_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "noisy",
            "echo 'Welcome to your shell!'\n\
             printf '\\033]0;title\\007'\n\
             read answer\n\
             export FROM_RC=yes\n\
             export JAVA_HOME=/opt/java\n\
             export PATH=/rc/bin:$PATH\n\
             eval \"$last\"\n\
             echo 'bye from .zlogout'",
        );
        let resolved = resolve_with(&shell, NEVER).expect("resolves");
        assert!(
            resolved.env.path.starts_with("/rc/bin:"),
            "{}",
            resolved.env.path
        );
        assert_eq!(var(&resolved.env, "FROM_RC"), Some("yes"));
        assert_eq!(var(&resolved.env, "JAVA_HOME"), Some("/opt/java"));
        assert_eq!(var(&resolved.env, MARKER_ENV), None, "marker is denylisted");
        assert_eq!(resolved.flags, "-i -l -c");
    }

    #[test]
    fn probe_sees_the_marker_variable() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "marker",
            "[ \"$KAGE_RESOLVING_ENVIRONMENT\" = 1 ] && export SAW_MARKER=1\neval \"$last\"",
        );
        let resolved = resolve_with(&shell, NEVER).expect("resolves");
        assert_eq!(var(&resolved.env, "SAW_MARKER"), Some("1"));
    }

    #[test]
    fn exec_in_interactive_rc_falls_back_to_login_only() {
        let dir = tempfile::tempdir().unwrap();
        // Mimics an unguarded `exec tmux` in .zshrc: the interactive form
        // never runs the payload; the non-interactive login form does.
        let shell = fake_shell(
            dir.path(),
            "exec-tmux",
            "case \" $* \" in *' -i '*) exec /usr/bin/false ;; esac\neval \"$last\"",
        );
        let started = Instant::now();
        let resolved = resolve_with(&shell, NEVER).expect("resolves");
        assert_eq!(resolved.flags, "-l -c");
        assert!(started.elapsed() < WELL_BEFORE);
    }

    #[test]
    fn shell_that_prints_nothing_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "silent", "exit 0");
        let started = Instant::now();
        assert_eq!(
            resolve_with(&shell, NEVER).err(),
            Some(ProbeError::NoEnvironment)
        );
        assert!(started.elapsed() < WELL_BEFORE);
    }

    #[test]
    fn exited_shell_with_lingering_job_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        // A background job inherits stdout and outlives the shell, so EOF
        // doesn't come for 30 s (> WELL_BEFORE); we must not wait for it —
        // neither after the end marker nor when the shell exits without
        // printing anything.
        let printing = fake_shell(dir.path(), "lingering", "sleep 30 &\neval \"$last\"");
        let started = Instant::now();
        assert!(resolve_with(&printing, NEVER).is_ok());
        assert!(started.elapsed() < WELL_BEFORE, "{:?}", started.elapsed());

        let silent = fake_shell(dir.path(), "lingering-silent", "sleep 30 &\nexit 0");
        let started = Instant::now();
        assert_eq!(
            resolve_with(&silent, NEVER).err(),
            Some(ProbeError::NoEnvironment)
        );
        assert!(started.elapsed() < WELL_BEFORE, "{:?}", started.elapsed());
    }

    #[test]
    fn hung_shell_times_out_and_its_process_group_dies() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("bg.pid");
        let shell = fake_shell(
            dir.path(),
            "hang",
            &format!("sleep 300 &\necho $! > '{}'\nsleep 300", pid_file.display()),
        );
        let started = Instant::now();
        // Long enough for the fake shell to reach `echo $!` on a loaded
        // runner: the first exec of a just-written script can take a
        // while on macOS (1.5 s flaked under the full parallel suite).
        assert_eq!(
            resolve_with(&shell, Duration::from_secs(4)).err(),
            Some(ProbeError::TimedOut)
        );
        assert!(started.elapsed() < WELL_BEFORE);

        // The rc file's background job was in the shell's group: killed.
        let bg: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone_by = Instant::now() + WELL_BEFORE;
        while running(bg) {
            assert!(Instant::now() < gone_by, "background job {bg} survived");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Alive and not a zombie. A killed orphan is reaped by init/launchd —
    /// or by whatever subreaper a CI runner uses, possibly late — so a
    /// zombie counts as dead.
    fn running(pid: i32) -> bool {
        match Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
        {
            Ok(out) => {
                let stat = String::from_utf8_lossy(&out.stdout);
                let stat = stat.trim();
                !stat.is_empty() && !stat.starts_with('Z')
            }
            Err(_) => nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok(),
        }
    }

    #[test]
    fn endless_output_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "yes", "exec yes kage");
        let started = Instant::now();
        let err = resolve_with(&shell, NEVER).err();
        assert_eq!(err, Some(ProbeError::TooMuchOutput));
        assert!(err.unwrap().killed());
        assert!(started.elapsed() < WELL_BEFORE);
    }

    /// The `env -0` fallback, end to end through the real payload: the
    /// fake shell swaps `/usr/bin/env -0` for a command that fails the way
    /// macOS 11's `env` does (usage on stderr, exit 1, nothing on stdout).
    #[test]
    fn env_without_dash_zero_falls_back_to_newline_output() {
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("old-env");
        std::fs::write(
            &stub,
            "#!/bin/sh\necho 'env: illegal option -- 0' >&2\necho 'usage: env [-iv] ...' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shell = fake_shell(
            dir.path(),
            "old-macos",
            &format!(
                "export MULTI='first\nsecond'\n\
                 export JAVA_HOME=/opt/java\n\
                 eval \"$(printf %s \"$last\" | sed 's#/usr/bin/env -0#{}#')\"",
                stub.display()
            ),
        );
        let resolved = resolve_with(&shell, NEVER).expect("falls back to plain env");
        assert_eq!(var(&resolved.env, "JAVA_HOME"), Some("/opt/java"));
        assert_eq!(var(&resolved.env, "MULTI"), Some("first\nsecond"));
        assert!(resolved
            .env
            .path
            .split(':')
            .any(|p| p == "/usr/bin" || p == "/bin"));
        assert_eq!(resolved.flags, "-i -l -c");
    }

    /// The `-0` path still wins when `env` supports it (every CI runner).
    #[test]
    fn env_dash_zero_keeps_newlines_in_values() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "modern",
            "export MULTI='a\nB=not-a-var'\neval \"$last\"",
        );
        let resolved = resolve_with(&shell, NEVER).expect("resolves");
        assert_eq!(var(&resolved.env, "MULTI"), Some("a\nB=not-a-var"));
        assert_eq!(var(&resolved.env, "B"), None);
    }

    #[test]
    fn stat_session_parses_tricky_command_names() {
        assert_eq!(
            stat_session("1234 (bash) S 1 1234 1234 0 -1 4194560"),
            Some(('S', 1234))
        );
        assert_eq!(stat_session("77 (a b) c) (d) Z 5 70 71 0"), Some(('Z', 71)));
        assert_eq!(stat_session("garbage"), None);
        assert_eq!(stat_session("1 (x) R 2"), None);
    }

    /// Job control in an rc file (`set -m` in ~/.bash_profile) puts every
    /// job in its own process group, out of `killpg`'s reach: the session
    /// sweep must still kill both the background and the foreground job.
    /// Real bash with a temp HOME, through a wrapper so the test process's
    /// own HOME is untouched.
    #[test]
    fn timeout_kills_job_control_jobs_in_their_own_process_groups() {
        let bash = Path::new("/bin/bash");
        if !is_usable_shell(bash) {
            eprintln!("no /bin/bash; skipping");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let bg_pid = home.path().join("bg.pid");
        let fg_pid = home.path().join("fg.pid");
        std::fs::write(
            home.path().join(".bash_profile"),
            "set -m\n\
             sleep 309 &\n\
             echo $! > \"$HOME/bg.pid\"\n\
             /bin/sh -c 'echo $$ > \"$HOME/fg.pid\"; exec sleep 310'\n",
        )
        .unwrap();
        let shell = fake_shell(
            home.path(),
            "bash",
            &format!("HOME='{}' exec /bin/bash \"$@\"", home.path().display()),
        );
        let started = Instant::now();
        // Long enough for a loaded runner to get bash through the rc file
        // (both pid files are written well before the deadline).
        let err = resolve_with(&shell, Duration::from_secs(4)).err();
        assert_eq!(err, Some(ProbeError::TimedOut));
        assert!(started.elapsed() < WELL_BEFORE);

        let read_pid = |p: &Path| -> i32 {
            std::fs::read_to_string(p)
                .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
                .trim()
                .parse()
                .unwrap()
        };
        let (bg, fg) = (read_pid(&bg_pid), read_pid(&fg_pid));
        let gone_by = Instant::now() + WELL_BEFORE;
        while running(bg) || running(fg) {
            if Instant::now() >= gone_by {
                let survivors = format!(
                    "bg {bg} running={}, fg {fg} running={}",
                    running(bg),
                    running(fg)
                );
                // Don't leave them behind for the next run either.
                for pid in [bg, fg] {
                    let _ = nix::sys::signal::kill(
                        nix::unistd::Pid::from_raw(pid),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }
                panic!("job-control jobs survived the timeout kill: {survivors}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Real login shell of whoever runs the tests. Ignored by default:
    /// it runs the developer's actual rc files (side effects, prompts for
    /// credentials, …). Run with `cargo test real_login_shell_smoke --
    /// --ignored`. Only invariants are checked when it works; a failure is
    /// printed, not fatal (the fake-shell tests above cover the failure
    /// paths deterministically).
    #[test]
    #[ignore = "runs the real login shell's rc files; opt in with --ignored"]
    fn real_login_shell_smoke() {
        let Some(shell) = select_shell() else {
            eprintln!("no usable login shell; skipping");
            return;
        };
        match resolve_with(&shell, PROBE_TIMEOUT) {
            Ok(resolved) => {
                assert!(
                    resolved
                        .env
                        .path
                        .split(':')
                        .any(|p| p == "/usr/bin" || p == "/bin"),
                    "{}",
                    resolved.env.path
                );
                assert!(resolved
                    .env
                    .vars
                    .iter()
                    .all(|(k, _)| policy::is_importable(k)));
            }
            Err(e) => eprintln!("real login shell {} failed: {e}", shell.display()),
        }
    }
}

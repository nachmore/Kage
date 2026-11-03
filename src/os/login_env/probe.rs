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
//! * **Hangs**: a hard deadline; on expiry the whole process group is
//!   SIGKILLed (background jobs an rc file started stay in the shell's
//!   group because a non-tty interactive shell has no job control).
//! * **Lingering background jobs holding stdout**: we stop reading as soon
//!   as the end marker arrives, or shortly after the shell exits, rather
//!   than waiting for EOF.
//! * **Endless output**: capped at [`MAX_OUTPUT`].
//! * **`exec` in an interactive rc** (`exec zsh`, unguarded `exec tmux`):
//!   no markers come back, so we retry once as a non-interactive login
//!   shell, which skips `.zshrc` / `.bashrc`.
//!
//! The probe sets `KAGE_RESOLVING_ENVIRONMENT=1` so rc files can skip
//! work that only makes sense in a real terminal.

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
/// no embedded quotes, which every supported shell treats literally.
/// `env` dumps exactly the *exported* variables, which is what a child
/// process would inherit; `-0` keeps values with newlines intact.
pub(super) fn probe_attempts(kind: ShellKind, start: &str, end: &str) -> Vec<Vec<String>> {
    let payload = format!("/bin/sh -c 'printf %s $0; /usr/bin/env -0; printf %s $1' {start} {end}");
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
    NoEnvironment,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "could not start the shell: {e}"),
            Self::TimedOut => write!(f, "timed out"),
            Self::NoEnvironment => write!(f, "the shell printed no environment"),
        }
    }
}

/// A resolved environment plus how it was obtained (for the log line).
pub(super) struct Resolved {
    pub env: LoginEnv,
    pub flags: String,
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
    let home = dirs::home_dir();

    for args in probe_attempts(shell_kind(shell), &start, &end) {
        let stdout = run_probe(shell, &args, home.as_deref(), &end, deadline)?;
        if let Some(block) = policy::extract_block(&stdout, &start, &end) {
            let env = policy::build_login_env(policy::parse_env_block(block))
                .ok_or(ProbeError::NoEnvironment)?;
            let flags = args[..args.len() - 1].join(" ");
            return Ok(Resolved { env, flags });
        }
        // No markers: probably an `exec` in an interactive rc. Retry with
        // the next (less interactive) form, if any.
    }
    Err(ProbeError::NoEnvironment)
}

/// Spawn one probe and collect its stdout until the end marker, EOF, the
/// shell's exit (+ grace), the output cap, or `deadline`. The child is
/// always reaped; on the failure paths its whole process group is killed.
fn run_probe(
    shell: &Path,
    args: &[String],
    cwd: Option<&Path>,
    end_marker: &str,
    deadline: Instant,
) -> Result<Vec<u8>, ProbeError> {
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
    // and group leader (pgid == pid, so `killpg` reaches everything it
    // started) with no controlling terminal.
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
                }
            }
            Stop::Overflow | Stop::TimedOut => kill_and_reap(&mut child),
        }
    }

    match stop {
        Stop::TimedOut => Err(ProbeError::TimedOut),
        _ => Ok(buf),
    }
}

/// SIGKILL the child's whole process group, then reap the child. Only
/// valid while the child is unreaped (its pid — and so the group id —
/// can't be recycled until then).
fn kill_and_reap(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        if pid > 0 {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
    let _ = child.kill();
    let _ = child.wait();
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
        assert_eq!(
            resolve_with(&shell, Duration::from_millis(1500)).err(),
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
        assert_eq!(
            resolve_with(&shell, NEVER).err(),
            Some(ProbeError::NoEnvironment)
        );
        assert!(started.elapsed() < WELL_BEFORE);
    }

    /// Real login shell of whoever runs the tests. CI runners and dev
    /// boxes have arbitrary rc files, so only check invariants when it
    /// works; a failure is printed, not fatal (the fake-shell tests above
    /// cover the failure paths deterministically).
    #[test]
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

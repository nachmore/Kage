//! The user's login-shell environment, for child processes that run
//! user-installed tools.
//!
//! # Problem
//!
//! GUI apps on macOS launched from Finder / Dock / `open` (which is how
//! Kage relaunches after an update) inherit launchd's minimal environment:
//! `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, no `LANG`, nothing from `.zshrc`.
//! Many Linux desktop sessions are similar (`.profile` maybe, `.bashrc`
//! never). The ACP agent (`kiro-cli acp`, …) inherits that, and so do the
//! MCP servers it spawns from `mcp.json` — so `"command": "builder-mcp"`
//! (installed in `~/.toolbox/bin`) fails to start even though it works in
//! a terminal. A node-based CLI found by absolute path still dies on its
//! `#!/usr/bin/env node` shebang.
//!
//! # Why the whole environment, not just PATH
//!
//! PATH alone fixes "command not found", but MCP servers and the tools
//! they shell out to also depend on what rc files export: `JAVA_HOME`,
//! `GOPATH`, version-manager roots (`NVM_DIR`, `PYENV_ROOT`, `ASDF_DIR` —
//! some shims refuse to run without them), corporate proxy and CA settings
//! (`HTTPS_PROXY` / `NO_PROXY` / `SSL_CERT_FILE` / `NODE_EXTRA_CA_CERTS`),
//! `AWS_PROFILE` / `AWS_REGION`, `LANG`. The goal is "behaves like it does
//! in a terminal", which is what VS Code's "resolve shell environment"
//! does too. Unlike VS Code we don't let the shell *override* anything:
//! see [`policy`] — add-missing only, PATH merged, explicit denylist, size
//! limits, and nothing set on the `Command` itself is ever touched.
//!
//! # Why per-`Command`, not `std::env::set_var`
//!
//! Resolution finishes on a background thread while Tauri, Tokio, and the
//! webviews are already running; `setenv` racing `getenv` on another
//! thread is undefined behaviour on Unix. So Kage's own environment stays
//! as launched and [`apply`] overlays the login environment onto each
//! child that runs a user tool. `std::process::Command` resolves a bare
//! program name against the PATH set *on the Command*, not the parent's
//! (documented on `Command::new` — "the search path to be used may be
//! controlled by setting the PATH environment variable on the Command" —
//! with caveats only on Windows), so `Command::new("kiro-cli")` finds
//! `~/.local/bin/kiro-cli` once [`apply`] has run. The
//! `bare_program_resolves_via_child_path` test pins this on every Unix CI
//! runner, with and without the agent spawn's `setsid` pre_exec.
//!
//! # When it runs
//!
//! * [`prewarm`] at startup kicks off one probe of `$SHELL -i -l` on a
//!   background thread ([`probe`] for the gory details); the result is
//!   cached for the life of the process.
//! * [`apply`] / [`overlay`] wait up to [`SPAWN_WAIT`] for an in-flight
//!   probe — never on the main (UI) thread, which only ever takes what's
//!   ready. If it isn't ready (or failed), the child gets its inherited
//!   PATH plus well-known user bin dirs; a later spawn picks up the
//!   resolved environment once it lands.
//! * Launched from a terminal (any stdio stream is a TTY), Kage already
//!   has the user's real environment — possibly more specific than a
//!   fresh login shell (an active venv, nix-shell, direnv) — so nothing
//!   is probed or changed.
//! * `KAGE_LOGIN_SHELL_ENV=0` (also `off`/`false`) disables all of this;
//!   `=1` (also `on`/`force`) probes even when launched from a terminal.
//!
//! Windows: no-op. Explorer-launched processes already inherit the full
//! user + system environment from the registry.

use std::process::Command;

#[cfg(unix)]
mod policy;
#[cfg(unix)]
mod probe;

/// Longest a spawn waits for an in-flight probe before falling back. The
/// launch-time agent spawn races the prewarm, and healthy rc files take
/// 0.1–2 s; anything slower is pathological and we'd rather connect with
/// the fallback PATH (logged) than stall the first session.
#[cfg(unix)]
pub const SPAWN_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Start resolving the login environment in the background so the first
/// agent spawn doesn't wait for the user's rc files. Idempotent; cheap.
pub fn prewarm() {
    #[cfg(unix)]
    imp::start();
}

/// Overlay the login environment onto `cmd` (see the module docs). Call
/// for every spawn of a user-installed tool: the agent, `which` lookups
/// that mirror its resolution, version probes, npm, python, …. Not for
/// OS binaries with absolute or system paths (`osascript`, `open`, …).
/// Call it *after* any `cmd.env(...)` the caller needs: those win. Don't
/// combine with `env_clear()` (cleared names aren't visible to us).
///
/// Only blocks while the startup probe is still in flight (the first
/// second or so after launch), for at most [`SPAWN_WAIT`], and never on
/// the main thread; afterwards it's a lookup plus a few `getenv`s.
pub fn apply(cmd: &mut Command) {
    #[cfg(unix)]
    {
        let explicit: Vec<String> = cmd
            .get_envs()
            .filter_map(|(k, _)| k.to_str().map(str::to_owned))
            .collect();
        for (k, v) in imp::overlay(&|name| explicit.iter().any(|e| e == name)) {
            cmd.env(k, v);
        }
    }
    #[cfg(not(unix))]
    let _ = cmd;
}

/// The same overlay as `(name, value)` pairs, for spawn helpers that take
/// an env list instead of a `Command` (e.g. the kage-core script runner).
/// `explicit` lists the names the caller sets itself; those are skipped.
pub fn overlay(explicit: &[&str]) -> Vec<(String, String)> {
    #[cfg(unix)]
    {
        imp::overlay(&|name| explicit.contains(&name))
    }
    #[cfg(not(unix))]
    {
        let _ = explicit;
        Vec::new()
    }
}

#[cfg(unix)]
mod imp {
    use super::policy::{self, LoginEnv};
    use super::probe;
    use log::{info, warn};
    use std::io::IsTerminal;
    use std::sync::{Arc, Condvar, Mutex, Once, OnceLock};
    use std::time::{Duration, Instant};

    enum Phase {
        Idle,
        Resolving,
        Ready(Option<Arc<LoginEnv>>),
    }

    static PHASE: Mutex<Phase> = Mutex::new(Phase::Idle);
    static READY: Condvar = Condvar::new();

    /// Whether to touch child environments at all. Decided once.
    fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            let setting = std::env::var("KAGE_LOGIN_SHELL_ENV")
                .unwrap_or_default()
                .to_ascii_lowercase();
            match setting.as_str() {
                "0" | "off" | "false" | "no" => {
                    info!("login env: disabled by KAGE_LOGIN_SHELL_ENV");
                    false
                }
                "1" | "on" | "true" | "yes" | "force" => true,
                _ => {
                    let tty = std::io::stdin().is_terminal()
                        || std::io::stdout().is_terminal()
                        || std::io::stderr().is_terminal();
                    if tty {
                        info!(
                            "login env: launched from a terminal; children keep the inherited environment"
                        );
                    }
                    !tty
                }
            }
        })
    }

    pub(super) fn start() {
        if !enabled() {
            return;
        }
        {
            let mut phase = PHASE.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(*phase, Phase::Idle) {
                return;
            }
            *phase = Phase::Resolving;
        }
        let spawned = std::thread::Builder::new()
            .name("kage-login-env".into())
            // A panic must still publish a result, or every later spawn
            // would sit out SPAWN_WAIT.
            .spawn(|| finish(std::panic::catch_unwind(resolve).unwrap_or(None)));
        if let Err(e) = spawned {
            warn!("login env: could not start resolver thread: {e}");
            finish(None);
        }
    }

    fn finish(result: Option<LoginEnv>) {
        *PHASE.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Ready(result.map(Arc::new));
        READY.notify_all();
    }

    fn resolve() -> Option<LoginEnv> {
        let Some(shell) = probe::select_shell() else {
            warn!("login env: no usable login shell; using the inherited PATH");
            return None;
        };
        let started = Instant::now();
        match probe::resolve_with(&shell, probe::PROBE_TIMEOUT) {
            Ok(resolved) => {
                // Names only — values can be credentials. PATH is safe to
                // log and is what support needs to see.
                let new: Vec<&str> = resolved
                    .env
                    .vars
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .filter(|k| std::env::var_os(k).is_none())
                    .collect();
                info!(
                    "login env: resolved via {} {} in {} ms; PATH={}; importing {} variable(s): {}",
                    shell.display(),
                    resolved.flags,
                    started.elapsed().as_millis(),
                    resolved.env.path,
                    new.len(),
                    new.join(", ")
                );
                Some(resolved.env)
            }
            Err(e) => {
                warn!(
                    "login env: {} {e} after {} ms; children get the inherited PATH plus well-known bin dirs",
                    shell.display(),
                    started.elapsed().as_millis()
                );
                None
            }
        }
    }

    /// The resolved environment, waiting up to [`super::SPAWN_WAIT`] for
    /// an in-flight probe (no waiting on the main thread).
    fn current() -> Option<Arc<LoginEnv>> {
        start();
        // std names the thread running `main` "main"; Tauri's event loop
        // lives there.
        let wait = if std::thread::current().name() == Some("main") {
            Duration::ZERO
        } else {
            super::SPAWN_WAIT
        };
        let phase = PHASE.lock().unwrap_or_else(|e| e.into_inner());
        let (phase, _) = READY
            .wait_timeout_while(phase, wait, |p| matches!(p, Phase::Resolving))
            .unwrap_or_else(|e| e.into_inner());
        match &*phase {
            Phase::Ready(env) => env.clone(),
            _ => {
                if !wait.is_zero() {
                    static WARNED: Once = Once::new();
                    WARNED.call_once(|| {
                        warn!(
                            "login env: still resolving after {} s; spawning with the fallback PATH",
                            wait.as_secs()
                        )
                    });
                }
                None
            }
        }
    }

    /// Existing well-known user bin dirs, computed once.
    fn fallback_dirs() -> &'static [String] {
        static DIRS: OnceLock<Vec<String>> = OnceLock::new();
        DIRS.get_or_init(|| {
            policy::fallback_dir_candidates(dirs::home_dir().as_deref())
                .into_iter()
                .filter(|d| d.is_dir())
                .filter_map(|d| d.into_os_string().into_string().ok())
                .collect()
        })
    }

    pub(super) fn overlay(explicit: &dyn Fn(&str) -> bool) -> Vec<(String, String)> {
        if !enabled() {
            return Vec::new();
        }
        let login = current();
        policy::plan_overlay(
            login.as_deref(),
            &|k| std::env::var_os(k),
            explicit,
            fallback_dirs(),
        )
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    /// Goal 2 evidence: std resolves a bare program name against the PATH
    /// set on the `Command`, not Kage's own — on both macOS and Linux, and
    /// with the agent spawn's `setsid` pre_exec in play.
    #[test]
    fn bare_program_resolves_via_child_path() {
        use std::os::unix::process::CommandExt;
        let dir = tempfile::tempdir().unwrap();
        let name = format!("kage-fake-tool-{}", std::process::id());
        let tool = dir.path().join(&name);
        std::fs::write(&tool, "#!/bin/sh\necho resolved-ok\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let parent_path = std::env::var("PATH").unwrap_or_default();
        assert!(!parent_path
            .split(':')
            .any(|p| p == dir.path().to_str().unwrap()));

        let child_path = format!("{}:/usr/bin:/bin", dir.path().display());
        for with_setsid in [false, true] {
            let mut cmd = Command::new(&name);
            cmd.env("PATH", &child_path);
            if with_setsid {
                // SAFETY: setsid is async-signal-safe.
                unsafe {
                    cmd.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
            }
            let out = cmd.output().expect("bare name resolves via child PATH");
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "resolved-ok");
        }

        // And without the override it is genuinely not findable.
        assert!(Command::new(&name).output().is_err());
    }

    #[test]
    fn overlay_never_touches_explicit_names() {
        let planned = super::overlay(&["PATH", "JAVA_HOME"]);
        assert!(planned.iter().all(|(k, _)| k != "PATH" && k != "JAVA_HOME"));
    }
}

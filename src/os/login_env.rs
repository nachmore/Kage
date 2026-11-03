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
//! see [`policy`] — add-missing only, PATH merged, explicit denylist, no
//! temporary AWS credentials, size limits, and nothing set on the
//! `Command` itself is ever touched.
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
//!   background thread ([`probe`] for the gory details).
//! * [`apply`] / [`overlay`] wait for that *first* probe only, never on
//!   the main (UI) thread, and only until [`SPAWN_WAIT`] after it started
//!   (a deadline shared by every caller, so nobody waits twice). Until it
//!   lands, or if it failed, the child gets its inherited PATH plus the
//!   well-known user bin dirs that exist.
//! * Re-probes ([`schedule`]): after a failure, a spawn at least a minute
//!   later (backing off to 30 min) starts one background retry; an agent
//!   (re)spawn ([`apply_agent`]) refreshes a result older than 30 min so a
//!   newly installed tool or an edited rc file is picked up without a
//!   restart. Nobody waits for either: the cached result keeps serving.
//! * Blocking: [`apply`] can block for up to [`SPAWN_WAIT`] early in a
//!   launch, so call it from a blocking context (`spawn_blocking`, a
//!   std thread) — never from an `async` body on a Tokio worker. The
//!   agent transport calls [`wait_ready`] *before* taking its connect
//!   lock so the lock is never held while waiting.
//!
//! # Turning it off
//!
//! * **Settings → Agents → "Use my shell's environment for agent tools"**
//!   (`acp.agent.import_login_shell_env`, default on), pushed in via
//!   [`set_enabled`] at startup and on every config save. Off means every
//!   child keeps exactly the inherited environment. Takes effect on the
//!   next spawn; turning it on starts the probe right away if none ran.
//! * Launched from a terminal (any stdio stream is a TTY), Kage already
//!   has the user's real environment — possibly more specific than a
//!   fresh login shell (an active venv, nix-shell, direnv) — so nothing
//!   is probed or changed.
//! * `KAGE_LOGIN_SHELL_ENV=0` (also `off`/`false`/`no`) disables all of
//!   this regardless of the setting; `=1` (also `on`/`true`/`yes`/`force`)
//!   enables it regardless of the setting, even from a terminal.
//! * Unit tests (`cfg(test)`) never probe the developer's real shell
//!   unless `KAGE_LOGIN_SHELL_ENV=1`; probe tests call
//!   `probe::resolve_with` with fake shells instead.
//!
//! rc files can test `KAGE_RESOLVING_ENVIRONMENT=1` to skip work that
//! doesn't belong in a background probe; a probe slower than
//! [`SLOW_PROBE`] (or killed) logs one line pointing at both switches.
//!
//! Windows: no-op. Explorer-launched processes already inherit the full
//! user + system environment from the registry.

use std::process::Command;

#[cfg(unix)]
mod policy;
#[cfg(unix)]
mod probe;
#[cfg(unix)]
mod schedule;

/// Longest a spawn waits for the first probe, measured from when that
/// probe started. The launch-time agent spawn races the prewarm, and
/// healthy rc files take 0.1–2 s; anything slower is pathological and
/// we'd rather connect with the fallback PATH (logged) than stall the
/// first session.
#[cfg(unix)]
pub const SPAWN_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// A probe slower than this logs a hint about the setting and the
/// `KAGE_RESOLVING_ENVIRONMENT` guard.
#[cfg(unix)]
pub const SLOW_PROBE: std::time::Duration = std::time::Duration::from_secs(3);

/// Environment variable that forces the feature off (`0`) or on (`1`),
/// overriding both the Settings toggle and the terminal check.
#[cfg(unix)]
pub const OVERRIDE_ENV: &str = "KAGE_LOGIN_SHELL_ENV";

/// Start resolving the login environment in the background so the first
/// agent spawn doesn't wait for the user's rc files. Also retries a failed
/// probe once its backoff has elapsed. Idempotent; cheap; never blocks.
pub fn prewarm() {
    #[cfg(unix)]
    imp::prewarm();
}

/// Apply the user's Settings toggle (`acp.agent.import_login_shell_env`).
/// Call at startup before [`prewarm`] and after every config change;
/// [`OVERRIDE_ENV`] still wins. Turning it on starts the probe in the
/// background if none has run. Never blocks.
pub fn set_enabled(enabled: bool) {
    #[cfg(unix)]
    imp::set_user_setting(enabled);
    #[cfg(not(unix))]
    let _ = enabled;
}

/// Record the saved Settings toggle at process start, before any window
/// (and so any invoke that might spawn a tool) exists: until then the
/// setting reads as on, which for a user who turned it off would run
/// their rc files anyway. Silent and never probes; [`set_enabled`] is for
/// later changes.
pub fn init_setting(enabled: bool) {
    #[cfg(unix)]
    imp::init_user_setting(enabled);
    #[cfg(not(unix))]
    let _ = enabled;
}

/// Block until the first probe has finished — at most until
/// [`SPAWN_WAIT`] after it started, and not at all on the main thread,
/// when disabled, or once any probe has finished. For callers that are
/// about to take a lock and then [`apply`]: waiting here first keeps the
/// lock hold time down to the spawn itself.
pub fn wait_ready() {
    #[cfg(unix)]
    imp::wait_ready();
}

/// Overlay the login environment onto `cmd` (see the module docs). Call
/// for every spawn of a user-installed tool: `which` lookups that mirror
/// the agent's resolution, version probes, npm, python, …. Not for OS
/// binaries with absolute or system paths (`osascript`, `open`, …). Call
/// it *after* any `cmd.env(...)` the caller needs: those win. Don't
/// combine with `env_clear()` (cleared names aren't visible to us).
///
/// Can block for up to [`SPAWN_WAIT`] while the first probe is in flight
/// (never on the main thread): call from a blocking context, not from an
/// `async` body on a Tokio worker.
pub fn apply(cmd: &mut Command) {
    #[cfg(unix)]
    apply_for(cmd, schedule::Purpose::Tool, true);
    #[cfg(not(unix))]
    let _ = cmd;
}

/// [`apply`] for the agent (re)spawn. Never waits: the spawn runs under
/// the transport's connect lock, and its caller already did the bounded
/// [`wait_ready`] before taking it (if the first probe started after that,
/// e.g. the toggle was just turned on, this spawn gets the fallback PATH).
/// Additionally kicks off a background refresh when the cached
/// environment is older than 30 minutes (the spawn itself still uses the
/// cached one).
pub fn apply_agent(cmd: &mut Command) {
    #[cfg(unix)]
    apply_for(cmd, schedule::Purpose::Agent, false);
    #[cfg(not(unix))]
    let _ = cmd;
}

#[cfg(unix)]
fn apply_for(cmd: &mut Command, purpose: schedule::Purpose, wait: bool) {
    let explicit = explicit_names(cmd);
    for (k, v) in imp::overlay(purpose, wait, &|name| explicit.iter().any(|e| e == name)) {
        cmd.env(k, v);
    }
}

/// Names `cmd` already sets *or removes* — both must be left alone.
#[cfg(unix)]
fn explicit_names(cmd: &Command) -> Vec<String> {
    cmd.get_envs()
        .filter_map(|(k, _)| k.to_str().map(str::to_owned))
        .collect()
}

/// The same overlay as `(name, value)` pairs, for spawn helpers that take
/// an env list instead of a `Command` (e.g. the kage-core script runner).
/// `explicit` lists the names the caller sets itself; those are skipped.
/// Blocks like [`apply`].
pub fn overlay(explicit: &[&str]) -> Vec<(String, String)> {
    #[cfg(unix)]
    {
        imp::overlay(schedule::Purpose::Tool, true, &|name| {
            explicit.contains(&name)
        })
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
    use super::schedule::{Purpose, Schedule};
    use log::{info, warn};
    use std::io::IsTerminal;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once, OnceLock};
    use std::time::Instant;

    static STATE: Mutex<Schedule<LoginEnv>> = Mutex::new(Schedule::new());
    /// Signalled whenever a probe finishes.
    static FINISHED: Condvar = Condvar::new();
    /// The Settings toggle. Default on, like the config field.
    static USER_SETTING: AtomicBool = AtomicBool::new(true);

    fn state() -> MutexGuard<'static, Schedule<LoginEnv>> {
        STATE.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `KAGE_LOGIN_SHELL_ENV`, read once: `Some(on)` when set to a
    /// recognised value.
    fn env_override() -> Option<bool> {
        static OVERRIDE: OnceLock<Option<bool>> = OnceLock::new();
        *OVERRIDE.get_or_init(|| {
            let setting = std::env::var(super::OVERRIDE_ENV)
                .unwrap_or_default()
                .to_ascii_lowercase();
            match setting.as_str() {
                "0" | "off" | "false" | "no" => {
                    info!("login env: disabled by KAGE_LOGIN_SHELL_ENV");
                    Some(false)
                }
                "1" | "on" | "true" | "yes" | "force" => {
                    info!("login env: forced on by KAGE_LOGIN_SHELL_ENV");
                    Some(true)
                }
                _ => None,
            }
        })
    }

    /// Whether any stdio stream was a terminal at launch. Decided once.
    fn launched_from_terminal() -> bool {
        static TTY: OnceLock<bool> = OnceLock::new();
        *TTY.get_or_init(|| {
            let tty = std::io::stdin().is_terminal()
                || std::io::stdout().is_terminal()
                || std::io::stderr().is_terminal();
            if tty {
                info!(
                    "login env: launched from a terminal; children keep the inherited environment"
                );
            }
            tty
        })
    }

    /// Whether to touch child environments at all, right now.
    pub(super) fn enabled() -> bool {
        match env_override() {
            Some(forced) => forced,
            // Unit tests must never probe the developer's real shell.
            None if cfg!(test) => false,
            None => USER_SETTING.load(Ordering::Relaxed) && !launched_from_terminal(),
        }
    }

    pub(super) fn init_user_setting(on: bool) {
        USER_SETTING.store(on, Ordering::Relaxed);
    }

    /// Off only because of the Settings toggle (not the env override, a
    /// terminal launch, or a test build).
    fn off_by_setting() -> bool {
        env_override().is_none()
            && !cfg!(test)
            && !USER_SETTING.load(Ordering::Relaxed)
            && !launched_from_terminal()
    }

    pub(super) fn set_user_setting(on: bool) {
        let was = USER_SETTING.swap(on, Ordering::Relaxed);
        if was != on {
            info!(
                "login env: {} in Settings (acp.agent.import_login_shell_env); applies to the next spawn",
                if on { "turned on" } else { "turned off" }
            );
            // Turned on with no probe yet (it was off at launch): start
            // one now so the next agent spawn rarely has to wait.
            if on {
                prewarm();
            }
        }
    }

    pub(super) fn prewarm() {
        if enabled() {
            use_state(Purpose::Tool, false);
        }
    }

    pub(super) fn wait_ready() {
        if enabled() {
            use_state(Purpose::Tool, may_wait());
        }
    }

    /// std names the thread running `main` "main"; Tauri's event loop
    /// lives there and must never wait.
    fn may_wait() -> bool {
        std::thread::current().name() != Some("main")
    }

    /// Record a use (starting a probe if the schedule says so), wait for
    /// the first probe if allowed, and return the current result.
    fn use_state(purpose: Purpose, may_wait: bool) -> Option<Arc<LoginEnv>> {
        let mut st = state();
        let now = Instant::now();
        if st.on_use(now, purpose) {
            // Why a probe that isn't the first one runs, for the log.
            let reprobe = st.is_reprobe().then(|| match st.resolved_at() {
                Some(at) if !st.last_probe_failed() => format!(
                    "refreshing a {}-minute-old result",
                    now.saturating_duration_since(at).as_secs() / 60
                ),
                _ => "retrying after a failed probe".to_string(),
            });
            let on_failure = if st.current().is_some() {
                "keep the previous login environment"
            } else {
                "get the inherited PATH plus well-known bin dirs"
            };
            drop(st);
            start_probe(reprobe, on_failure);
            st = state();
        }
        if may_wait {
            if let Some(deadline) = st.wait_deadline(super::SPAWN_WAIT) {
                let remaining = deadline.saturating_duration_since(Instant::now());
                st = FINISHED
                    .wait_timeout_while(st, remaining, |s| s.first_probe_in_flight())
                    .map(|(guard, _)| guard)
                    .unwrap_or_else(|e| e.into_inner().0);
                if st.first_probe_in_flight() {
                    static WARNED: Once = Once::new();
                    WARNED.call_once(|| {
                        warn!(
                            "login env: still resolving after {} s; spawning with the fallback PATH",
                            super::SPAWN_WAIT.as_secs()
                        )
                    });
                }
            }
        }
        st.current()
    }

    /// Run one probe on a background thread; the schedule already counts
    /// it as in flight, so it must always `finish`. `reprobe` says why a
    /// probe other than the first one runs; `on_failure` what children
    /// get if it fails (both only logged).
    fn start_probe(reprobe: Option<String>, on_failure: &'static str) {
        let spawned = std::thread::Builder::new()
            .name("kage-login-env".into())
            // A panic must still publish a result, or the schedule would
            // think a probe is running forever.
            .spawn(move || {
                let result = std::panic::catch_unwind(|| resolve(reprobe.as_deref(), on_failure))
                    .unwrap_or(None);
                finish(result);
            });
        if let Err(e) = spawned {
            warn!("login env: could not start resolver thread: {e}");
            finish(None);
        }
    }

    fn finish(result: Option<LoginEnv>) {
        state().finish(Instant::now(), result);
        FINISHED.notify_all();
    }

    fn resolve(reprobe: Option<&str>, on_failure: &str) -> Option<LoginEnv> {
        let Some(shell) = probe::select_shell() else {
            warn!("login env: no usable login shell; using the inherited PATH");
            return None;
        };
        if let Some(why) = reprobe {
            info!(
                "login env: {why}; re-resolving via {} in the background",
                shell.display()
            );
        }
        let started = Instant::now();
        let outcome = probe::resolve_with(&shell, probe::PROBE_TIMEOUT);
        let elapsed = started.elapsed();
        let killed = match &outcome {
            Ok(resolved) => resolved.killed,
            Err(e) => e.killed(),
        };
        if killed || elapsed > super::SLOW_PROBE {
            warn!(
                "login env: {} startup files {} after {} ms; skip slow or side-effecting rc-file \
                 snippets when KAGE_RESOLVING_ENVIRONMENT=1 is set, or turn this off in \
                 Settings > Agents > \"Use my shell's environment for agent tools\" \
                 (acp.agent.import_login_shell_env) or with KAGE_LOGIN_SHELL_ENV=0",
                shell.display(),
                if killed { "were killed" } else { "finished" },
                elapsed.as_millis()
            );
        }
        match outcome {
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
                    elapsed.as_millis(),
                    resolved.env.path,
                    new.len(),
                    new.join(", ")
                );
                Some(resolved.env)
            }
            Err(e) => {
                warn!(
                    "login env: {} {e} after {} ms; children {on_failure}",
                    shell.display(),
                    elapsed.as_millis(),
                );
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

    pub(super) fn overlay(
        purpose: Purpose,
        wait: bool,
        explicit: &dyn Fn(&str) -> bool,
    ) -> Vec<(String, String)> {
        if !enabled() {
            // The toggle is about running rc files. Appending the
            // well-known user bin dirs runs nothing, and without them a
            // Finder-launched Kage can't find `kiro-cli`, Homebrew's
            // python, … at all.
            if off_by_setting() {
                return policy::plan_overlay(
                    None,
                    &|k| std::env::var_os(k),
                    explicit,
                    fallback_dirs(),
                );
            }
            return Vec::new();
        }
        let login = use_state(purpose, wait && may_wait());
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

    /// Spawn, retrying ETXTBSY: on Linux a just-written fixture can't be
    /// exec'd while another test thread's fork still holds it open for
    /// writing. It clears within milliseconds.
    fn output_retrying_etxtbsy(cmd: &mut Command) -> std::io::Result<std::process::Output> {
        let mut attempt = 0;
        loop {
            match cmd.output() {
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && attempt < 40 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                other => return other,
            }
        }
    }

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
            let out = output_retrying_etxtbsy(&mut cmd).expect("bare name resolves via child PATH");
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "resolved-ok");
        }

        // And without the override it is genuinely not findable.
        let err = output_retrying_etxtbsy(&mut Command::new(&name)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn explicit_names_include_set_and_removed_variables() {
        let mut cmd = Command::new("/bin/true");
        cmd.env("PATH", "/x").env_remove("JAVA_HOME").env("A", "1");
        let mut names = super::explicit_names(&cmd);
        names.sort();
        assert_eq!(names, ["A", "JAVA_HOME", "PATH"]);
    }

    /// Unit tests never probe the developer's real shell: `enabled()` is
    /// off under `cfg(test)` unless forced, so `apply` is a no-op.
    #[test]
    fn disabled_under_test_unless_forced() {
        if std::env::var(super::OVERRIDE_ENV).is_ok() {
            eprintln!("{} is set; skipping", super::OVERRIDE_ENV);
            return;
        }
        assert!(!super::imp::enabled());
        let mut cmd = Command::new("/bin/true");
        cmd.env("A", "1");
        super::apply(&mut cmd);
        super::apply_agent(&mut cmd);
        assert_eq!(super::explicit_names(&cmd), ["A"]);
        assert!(super::overlay(&[]).is_empty());
        // Neither the setting nor prewarm/wait_ready change that.
        super::set_enabled(true);
        super::prewarm();
        super::wait_ready();
        assert!(!super::imp::enabled());
    }
}

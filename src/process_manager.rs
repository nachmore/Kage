use anyhow::{Context, Result};
use log::{info, warn};
use std::fs;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};

use crate::lock_ext::LockExt;
use crate::os;

/// Manages spawned CLI processes with cleanup on exit
pub struct ProcessManager {
    child: Arc<Mutex<Option<Child>>>,
    pid_file: PathBuf,
    pid: Option<u32>,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new() -> Self {
        let pid_file = Self::get_pid_file_path();
        Self {
            child: Arc::new(Mutex::new(None)),
            pid_file,
            pid: None,
        }
    }

    /// Get the path to the PID file
    fn get_pid_file_path() -> PathBuf {
        let mut path = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("kage");

        // Create directory if it doesn't exist
        if let Err(e) = fs::create_dir_all(&path) {
            warn!("Failed to create PID directory {:?}: {}", path, e);
        }

        path.push("spawned_cli.pid");
        path
    }

    /// Clean up any orphaned processes from previous runs
    pub fn cleanup_orphaned_processes() -> Result<()> {
        let pid_file = Self::get_pid_file_path();

        if !pid_file.exists() {
            info!("No PID file found, no orphaned processes to clean up");
            return Ok(());
        }

        match fs::read_to_string(&pid_file) {
            Ok(content) => {
                if let Ok(pid) = content.trim().parse::<u32>() {
                    info!("Found PID file with PID: {}", pid);

                    // Verify the PID still belongs to a kage-related process
                    // to avoid killing a recycled PID that now belongs to something else
                    match os::process::get_process_name(pid) {
                        Some(name) => {
                            let name_lower = name.to_lowercase();
                            let is_ours = name_lower.contains("Kage")
                                || name_lower.contains("node")
                                || name_lower.contains("npx");

                            if is_ours {
                                info!("PID {} is '{}' — killing orphaned process", pid, name);
                                if Self::kill_process(pid) {
                                    info!(
                                        "✅ Cleaned up orphaned process (PID: {}, name: {})",
                                        pid, name
                                    );
                                } else {
                                    warn!(
                                        "Failed to kill orphaned process (PID: {}, name: {})",
                                        pid, name
                                    );
                                }
                            } else {
                                info!("PID {} is '{}' — not a kage process, skipping kill (PID was recycled)", pid, name);
                            }
                        }
                        None => {
                            info!("PID {} is not running (already exited)", pid);
                        }
                    }
                }

                // Remove the PID file
                let _ = fs::remove_file(&pid_file);
                info!("PID file removed");
            }
            Err(e) => {
                warn!("Failed to read PID file: {}", e);
                let _ = fs::remove_file(&pid_file);
            }
        }

        Ok(())
    }

    /// Store a spawned child process
    pub fn store_process(&mut self, child: Child) -> Result<()> {
        let pid = child.id();
        info!("Storing process with PID: {}", pid);

        // Write PID to file
        fs::write(&self.pid_file, pid.to_string()).context("Failed to write PID file")?;

        self.pid = Some(pid);
        *self.child.lock_or_recover() = Some(child);

        info!("✅ Process registered for cleanup (PID: {})", pid);
        Ok(())
    }

    /// Kill a process by PID
    fn kill_process(pid: u32) -> bool {
        os::kill_process(pid)
    }

    /// Liveness of the managed child process.
    ///
    /// - `None` — no child is managed (TCP/remote mode, or nothing spawned yet).
    /// - `Some(true)` — the child is still running.
    /// - `Some(false)` — the child has exited (and is reaped here via `try_wait`).
    ///
    /// The transport's `is_connected()` only flips to false once the reader
    /// thread observes EOF; there's a brief window where the agent process has
    /// died but that flag hasn't flipped yet. This lets callers (notably the
    /// restart coalesce guard) detect a dead agent within that window instead
    /// of trusting a stale `connected=true`.
    pub fn child_liveness(&self) -> Option<bool> {
        let mut guard = self.child.lock_or_recover();
        match guard.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(None) => Some(true),
                Ok(Some(_status)) => Some(false),
                Err(e) => {
                    warn!("try_wait on managed child failed: {}", e);
                    Some(false)
                }
            },
            None => None,
        }
    }

    /// Terminate the managed process
    pub fn terminate(&mut self) {
        if let Some(mut child) = self.child.lock_or_recover().take() {
            let pid = child.id();
            info!("Terminating spawned process (PID: {})", pid);

            // Try graceful shutdown first
            let _ = child.kill();

            // Wait for process to exit (with timeout)
            let wait_result = std::thread::spawn(move || child.wait()).join();

            if wait_result.is_ok() {
                info!("✅ Process terminated gracefully");
            } else {
                warn!("Process may not have terminated cleanly");

                // Force kill if still running
                if let Some(pid) = self.pid {
                    Self::kill_process(pid);
                }
            }
        }

        // Clean up PID file
        if self.pid_file.exists() {
            let _ = fs::remove_file(&self.pid_file);
            info!("✅ PID file removed");
        }

        self.pid = None;
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        info!("ProcessManager dropping, cleaning up...");
        self.terminate();
    }
}

// --- Cross-platform signal-handler child-cleanup registry ---------------
//
// On Windows, the Job Object reaps every child we spawn when the parent
// exits — even on hard crash. macOS / Linux have no equivalent. The
// `graceful_shutdown` path covers tray-quit / `quit_app` / `restart_app`
// because it can reach the AppHandle and walk `ChildProcesses` directly,
// but signal-driven exits (SIGTERM, SIGINT, SIGQUIT, the Cmd+Shift+Q
// hotkey wired to terminate(), etc.) install at startup before Tauri
// builds, so they only saw the agent backend's `ProcessManager`. Anything
// stored in `ChildProcesses` (pocket-tts server + its in-flight pip
// install) was leaking on macOS / Linux when the user closed the app
// via SIGTERM.
//
// This registry lets each child-spawning site register a "kill me"
// closure once. The signal handler walks the list in registration
// order. The registry is static so signal handlers (installed before
// Tauri builds) can reach it without threading the AppHandle through.

type Killer = Box<dyn Fn() + Send + Sync + 'static>;

static CHILD_KILLERS: std::sync::LazyLock<Mutex<Vec<Killer>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

/// Register a closure that the signal handler will run on shutdown.
/// Call from the spawn site (e.g. pocket-tts launch) once the
/// child handle is available.
pub fn register_child_killer(kill: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut killers) = CHILD_KILLERS.lock() {
        killers.push(Box::new(kill));
    }
}

/// Run every registered killer. Used by the signal handler.
fn run_all_killers() {
    let killers = match CHILD_KILLERS.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    for kill in killers.iter() {
        // Each killer is best-effort. We don't unwind the registry on
        // failure — a stuck child shouldn't block the rest from being
        // cleaned up.
        kill();
    }
}

/// Test-only: drain the registry. Each test that exercises the
/// killer registry must drain at the start so closures registered by
/// previously-completed tests can't bleed into this test's
/// `run_all_killers()` sweep. The closures captured Arcs we no
/// longer hold, so dropping them is safe.
#[cfg(test)]
fn _drain_killers_for_tests() {
    if let Ok(mut killers) = CHILD_KILLERS.lock() {
        killers.clear();
    }
}

/// Install signal handlers for graceful shutdown.
///
/// The cleanup closure terminates the agent backend AND walks the
/// child-killer registry, so any subsystem that registered via
/// `register_child_killer` gets a chance to clean up before we exit.
pub fn install_signal_handlers(process_manager: Arc<Mutex<ProcessManager>>) {
    let cleanup = move || {
        // Agent backend first — it's the heaviest child and the one
        // most likely to be holding network sockets.
        if let Ok(mut pm) = process_manager.lock() {
            pm.terminate();
        }
        // Then everything that registered itself (pocket-tts server +
        // any in-flight install on macOS / Linux). On Windows this is
        // redundant with the Job Object but harmless — the kills will
        // all return "process not found" once the OS has reaped them.
        run_all_killers();
    };

    if let Err(e) = os::process::install_signal_handlers(cleanup) {
        warn!("Failed to install signal handlers: {}", e);
    }
}

#[cfg(test)]
mod child_killer_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Serialise the two tests in this module. They both touch the
    /// global `CHILD_KILLERS` static — closures registered by test A
    /// stay registered after A returns and fire again when test B
    /// calls `run_all_killers()`, polluting B's observation. The
    /// assertion in `killers_run_in_registration_order` was failing
    /// intermittently on macOS for exactly this reason (CI run
    /// 26566571823).
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// A callback registered via `register_child_killer` actually fires
    /// when `run_all_killers()` runs. Without this guarantee the
    /// macOS / Linux SIGTERM path silently leaves orphan children
    /// behind — the bug this whole subsystem exists to fix.
    #[test]
    fn registered_killer_runs_on_invocation() {
        let _guard = TEST_LOCK.lock_or_recover();
        _drain_killers_for_tests();
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        register_child_killer(move || {
            f.store(true, Ordering::SeqCst);
        });
        run_all_killers();
        assert!(fired.load(Ordering::SeqCst));
    }

    /// Killers fire in registration order. The signal handler invokes
    /// them as a single sweep; if the order silently changed, a child
    /// that depends on a sibling being killed first (e.g. install
    /// before server) would race.
    #[test]
    fn killers_run_in_registration_order() {
        let _guard = TEST_LOCK.lock_or_recover();
        _drain_killers_for_tests();
        let order = Arc::new(Mutex::new(Vec::<u32>::new()));
        for i in 100..103 {
            let o = order.clone();
            register_child_killer(move || {
                if let Ok(mut v) = o.lock() {
                    v.push(i);
                }
            });
        }
        run_all_killers();
        let after = order.lock_or_recover();
        // Find our window in the global order — the static is shared
        // with other tests, so we look for our marker values.
        let our_slice: Vec<u32> = after
            .iter()
            .copied()
            .filter(|n| (100..103).contains(n))
            .collect();
        assert_eq!(our_slice, vec![100, 101, 102]);
    }
}

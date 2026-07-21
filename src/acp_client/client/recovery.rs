use anyhow::Result;
use log::{info, warn};

use super::super::AcpClient;
use crate::lock_ext::LockExt;

/// How many spawn+initialize attempts a restart makes before giving up.
const RESTART_MAX_ATTEMPTS: u32 = 3;
/// Base backoff between restart attempts; doubles each retry.
const RESTART_BASE_DELAY_MS: u64 = 300;
/// A restart that succeeded this recently is treated as "good enough" for a
/// concurrent/rapid caller — it coalesces onto the fresh connection instead
/// of respawning again.
pub(crate) const RESTART_COOLDOWN_MS: u64 = 2000;

pub(crate) fn should_coalesce_restart(
    since_last_ok: Option<std::time::Duration>,
    transport_healthy: bool,
) -> bool {
    match since_last_ok {
        Some(elapsed) => {
            elapsed < std::time::Duration::from_millis(RESTART_COOLDOWN_MS) && transport_healthy
        }
        None => false,
    }
}

impl AcpClient {
    pub(crate) fn force_disconnect(&self) {
        self.transport.force_disconnect();
        *self.initialized.lock_or_recover() = false;
        self.clear_compaction_gate();
    }

    /// Tear down and rebuild the agent connection, with coalescing and retry.
    pub(crate) fn restart_connection(&self) -> Result<()> {
        let mut last_ok = self.restart_guard.lock_or_recover();
        if should_coalesce_restart(
            last_ok.map(|when| when.elapsed()),
            self.transport.is_healthy(),
        ) {
            info!("restart_connection: coalescing onto recent healthy restart");
            return Ok(());
        }

        info!("Restarting ACP connection");
        self.force_disconnect();
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..RESTART_MAX_ATTEMPTS {
            let delay = RESTART_BASE_DELAY_MS * 2u64.pow(attempt);
            std::thread::sleep(std::time::Duration::from_millis(delay));
            match self.try_connect_and_initialize() {
                Ok(()) => {
                    *last_ok = Some(std::time::Instant::now());
                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        "restart_connection attempt {}/{} failed: {}",
                        attempt + 1,
                        RESTART_MAX_ATTEMPTS,
                        e
                    );
                    self.force_disconnect();
                    last_err = Some(e);
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| anyhow::anyhow!("restart_connection failed with no recorded error")))
    }

    fn try_connect_and_initialize(&self) -> Result<()> {
        self.transport.connect()?;
        self.initialize()?;
        Ok(())
    }

    /// Block the current thread until compaction is finished (with a timeout).
    /// Returns true if we waited, false if compaction wasn't active.
    pub fn wait_for_compaction(&self) -> bool {
        let (lock, cvar) = &*self.compacting;
        let mut compacting = lock.lock_or_recover();
        if !*compacting {
            return false;
        }
        info!("Waiting for compaction to finish before sending prompt...");
        let total_timeout = std::time::Duration::from_secs(60);
        let slice = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();
        loop {
            if !*compacting {
                info!("Compaction finished, proceeding with prompt");
                return true;
            }
            if !self.transport.is_connected() {
                log::warn!("Compaction wait aborted — transport disconnected");
                *compacting = false;
                cvar.notify_all();
                return true;
            }
            let elapsed = start.elapsed();
            if elapsed >= total_timeout {
                log::warn!("Compaction wait timed out after 60s — sending anyway");
                return true;
            }
            let remaining = total_timeout - elapsed;
            let this_slice = remaining.min(slice);
            let (guard, _) = match cvar.wait_timeout(compacting, this_slice) {
                Ok(result) => result,
                Err(poisoned) => poisoned.into_inner(),
            };
            compacting = guard;
        }
    }
}

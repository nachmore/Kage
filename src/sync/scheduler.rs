//! The `kage-sync` thread and the handle the app talks to it through
//! (docs/design/SYNC.md §14). Requests travel over a channel and wait with
//! a timeout; a wake never blocks.
//!
//! C0 skeleton: the handle never starts a thread, and every request
//! answers `sync_unavailable`. WP-S3 writes the thread.

use super::engine::{EngineEnv, Wake};
use super::sources::Source;
use super::types::Origin;
use super::{unavailable, EngineReply, EngineRequest};
use crate::error::AppError;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// A cheap handle to the engine thread. Built on the Tauri Builder before
/// the thread exists.
#[derive(Clone, Default)]
pub struct SyncHandle {
    /// The channel to the engine thread, once it started.
    _inner: Arc<OnceLock<()>>,
}

impl SyncHandle {
    /// A handle with no thread yet: every request answers
    /// `sync_unavailable` until [`SyncHandle::start`] runs.
    pub fn pending() -> Self {
        Self::default()
    }

    /// Start the engine thread after `delay`. `make` runs on that thread.
    /// A second call is a no-op.
    pub fn start(
        &self,
        make: impl FnOnce() -> anyhow::Result<(EngineEnv, Vec<Box<dyn Source>>)> + Send + 'static,
        delay: Duration,
    ) {
        let _ = (make, delay);
    }

    /// Ask the engine and wait up to `timeout`.
    pub fn request(&self, req: EngineRequest, timeout: Duration) -> Result<EngineReply, AppError> {
        let _ = (req, timeout);
        Err(unavailable())
    }

    /// Wake the engine. Never blocks.
    pub fn wake(&self, w: Wake) {
        let _ = w;
    }

    /// Flush own edits before exit, within `budget`.
    pub fn flush_for_exit(&self, budget: Duration) {
        let _ = budget;
    }
}

static GLOBAL: OnceLock<SyncHandle> = OnceLock::new();

/// Make `h` the handle [`notify`] wakes. The first call wins.
pub fn install_global(h: SyncHandle) {
    let _ = GLOBAL.set(h);
}

/// A local change happened in `source` (a config save, a steering write,
/// extension data). A no-op until a handle is installed.
pub fn notify(source: &'static str, origin: Origin) {
    if let Some(h) = GLOBAL.get() {
        h.wake(Wake::Local { source, origin });
    }
}

//! The sync engine (docs/design/SYNC.md §8, §19.4): one tick captures,
//! ingests, merges, projects and publishes. It runs only on the `kage-sync`
//! thread and reaches the app only through the ports below.
//!
//! C0 skeleton: nothing opens. WP-S3 writes the engine; WP-S4 implements
//! the ports for the app.

use super::fsio::SyncFs;
use super::keystore::KeyStore;
use super::sources::Source;
use super::status::SyncStatus;
use super::types::{Origin, OsKind};
use super::{unavailable, EngineReply, EngineRequest, NOT_BUILT};
use crate::error::AppError;
use crate::os::cloud_files::CloudFiles;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// `config.json` as the engine sees it.
pub trait ConfigPort: Send + Sync {
    /// The current config and its revision.
    fn snapshot(&self) -> (u64, crate::config::Config);
    /// Save `next` if nothing else saved since `base_revision`. Returns the
    /// new revision.
    fn commit(
        &self,
        base_revision: u64,
        next: crate::config::Config,
        origin: Origin,
    ) -> Result<u64, CommitError>;
    /// Whether `Config::load` reset the file or some sections (§8.10).
    fn load_was_reset(&self) -> LoadReset;
    /// A newer build wrote `config.json`; saves are suspended.
    fn saves_suspended(&self) -> bool;
    fn installed(&self) -> Vec<crate::extensions::InstalledItem>;
}

/// Why a commit didn't save.
#[derive(Debug)]
pub enum CommitError {
    /// Another save came first. Capture, then try again.
    Stale,
    Suspended,
    Io(anyhow::Error),
}

/// What `Config::load` had to reset this run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoadReset {
    #[default]
    None,
    All,
    Sections(Vec<String>),
}

/// What the engine tells the app.
pub trait HostEvents: Send + Sync {
    fn status_changed(&self, s: &SyncStatus);
    fn conflicts_changed(&self, open: usize);
    /// Emitted before `config_updated`, with catalog paths.
    fn applied(&self, keys: &[String]);
    /// Keys and lists modes: `extension_data_changed` with source `sync`.
    fn extension_data_changed(&self, ext: &str, keys: &[String]);
    /// Collections mode: `extension_sync_changes`.
    fn extension_sync_changes(&self, ext: &str, collections: &[String], reason: &str);
    /// An OS notification for a state the user must act on (§13.8).
    fn notify_user(&self, state: &str);
    fn track(&self, event: &'static str, props: Vec<(&'static str, String)>);
}

/// Time and power, so tests can drive them.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
    fn power(&self) -> crate::os::power::PowerState;
}

/// Everything the engine needs. It never reads `crate::paths` globals, so
/// one test process can run several computers.
pub struct EngineEnv {
    /// `<data_local>/kage/sync/`.
    pub local_dir: PathBuf,
    pub os: OsKind,
    pub kage_version: String,
    pub fingerprint: String,
    pub fs: Arc<dyn SyncFs>,
    pub cloud: Arc<dyn CloudFiles>,
    pub keys: Arc<dyn KeyStore>,
    pub clock: Arc<dyn Clock>,
    pub events: Arc<dyn HostEvents>,
    pub config: Arc<dyn ConfigPort>,
}

/// Why a tick runs (§14.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Startup,
    Local {
        source: &'static str,
        origin: Origin,
    },
    Folder,
    Poll,
    Focus,
    Resume,
    Manual,
    Exit,
}

/// What one tick did, for logs and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    pub captured: usize,
    pub ingested_shards: usize,
    pub projected: usize,
    pub published: bool,
    pub new_conflicts: usize,
}

/// The engine. Owns the replica; lives on the `kage-sync` thread.
pub struct SyncEngine {
    _env: EngineEnv,
    _sources: Vec<Box<dyn Source>>,
}

impl SyncEngine {
    /// Read `device.json`, open the replica, and finish unfinished
    /// projections.
    pub fn open(env: EngineEnv, sources: Vec<Box<dyn Source>>) -> anyhow::Result<Self> {
        let _ = (env, sources);
        anyhow::bail!(NOT_BUILT)
    }

    pub fn tick(&mut self, wake: Wake) -> TickReport {
        let _ = wake;
        TickReport::default()
    }

    /// Capture and publish own edits only, within `budget`. No ingest.
    pub fn flush_for_exit(&mut self, budget: Duration) {
        let _ = budget;
    }

    pub fn handle(&mut self, req: EngineRequest) -> Result<EngineReply, AppError> {
        let _ = req;
        Err(unavailable())
    }

    pub fn status(&self) -> SyncStatus {
        SyncStatus::unavailable()
    }
}

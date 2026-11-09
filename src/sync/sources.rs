//! Sources: the parts of Kage that sync plugs into (docs/design/SYNC.md
//! §19.4). Each reads its local files as records and writes merged records
//! back. WP-S3 writes `config`, `lists`, `steering` and `chats`; WP-S5 the
//! extension source.
//!
//! Conventions every source follows, so the engine can stay generic:
//! - A record's fields are its synced fields only. Fields that stay on
//!   this computer are left out of [`LocalRecord::fields`]; the source
//!   keeps them from its own copy when it writes the record back.
//! - A list keeps its order in the field [`ORDER`]: [`Source::read_local`]
//!   reports each item's position in the local list as a number, and a
//!   [`LocalWrite`] carries the merged fractional key as a string. The
//!   engine turns local positions into keys that move as few items as
//!   possible, so two reorders on two computers merge. A write to a list
//!   namespace always carries every record that exists here, so the source
//!   can rebuild the whole list in `(order, id)` order.
//! - [`LocalRecord::avail`] is where a *new* item exists by default (`all`
//!   or an OS name). The engine writes it once, when the record is new.
//! - A field may hold [`unchanged`] to say "the same value you last saw":
//!   the engine skips it without looking at it. A source with large values
//!   (chat transcripts) uses it so it doesn't load what didn't change.

pub mod chats;
pub mod config;
pub mod lists;
pub mod steering;

use super::crypto::Keys;
use super::engine::ConfigPort;
use super::replica::ExtStore;
use super::types::{Category, DefaultScope, DeviceId, Origin, Structure};
use super::{ExtReply, ExtRequest};
use crate::error::AppError;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub use super::split::ORDER;

/// The marker of [`unchanged`].
pub const UNCHANGED_KEY: &str = "$unchanged";

/// A field value that means "the value the engine last saw".
pub fn unchanged() -> serde_json::Value {
    serde_json::json!({ UNCHANGED_KEY: true })
}

/// Whether `v` is [`unchanged`].
pub fn is_unchanged(v: &serde_json::Value) -> bool {
    v.as_object().is_some_and(|o| {
        o.len() == 1 && o.get(UNCHANGED_KEY) == Some(&serde_json::Value::Bool(true))
    })
}

/// One namespace a source owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NsSpec {
    pub ns: String,
    pub category: Category,
    /// Records carry it; an older reader doesn't project a newer schema.
    pub schema: u32,
    /// Other computers republish what they received (§7.1).
    pub echo: bool,
    pub structure: Structure,
    pub default_scope: DefaultScope,
    /// Values go in blobs (up to 4 MiB).
    pub large: bool,
    /// The field that names a record in the conflicts list.
    pub label_field: Option<String>,
}

/// A record as the local files hold it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LocalRecord {
    pub fields: serde_json::Map<String, serde_json::Value>,
    /// List items: `all`, or one OS.
    pub avail: Option<String>,
}

/// Every record a source holds locally, by `(ns, id)`. `None` = absent.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LocalState {
    pub records: BTreeMap<(String, String), Option<LocalRecord>>,
}

impl LocalState {
    /// The record `ns`/`id`, if the files hold it.
    pub fn get(&self, ns: &str, id: &str) -> Option<&LocalRecord> {
        self.records
            .get(&(ns.to_string(), id.to_string()))
            .and_then(Option::as_ref)
    }

    pub fn put(&mut self, ns: &str, id: &str, r: LocalRecord) {
        self.records
            .insert((ns.to_string(), id.to_string()), Some(r));
    }
}

/// One merged value to write into the local files. `None` removes it.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalWrite {
    pub ns: String,
    pub id: String,
    pub value: Option<LocalRecord>,
}

/// The result of [`Source::project`].
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectResult {
    /// Written. `read_back` is what the files hold now.
    Applied { read_back: LocalState },
    /// The files changed underneath (a save raced the projection). The
    /// engine captures first and tries again.
    Stale,
}

/// A part of Kage that syncs.
pub trait Source: Send {
    fn id(&self) -> &'static str;
    fn specs(&self) -> Vec<NsSpec>;
    fn read_local(&mut self) -> anyhow::Result<LocalState>;
    fn project(&mut self, writes: Vec<LocalWrite>, origin: Origin)
        -> anyhow::Result<ProjectResult>;
    /// Collections-mode calls from extensions. Default: not handled.
    fn request(
        &mut self,
        _ctx: &mut dyn CaptureCtx,
        _req: &ExtRequest,
    ) -> Option<Result<ExtReply, AppError>> {
        None
    }
}

/// What one capture did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureResult {
    Unchanged,
    Written,
    /// Refused, with a reason code such as `sensitive_field`.
    Rejected(&'static str),
}

/// What the engine lends a source while it handles a request.
pub trait CaptureCtx {
    /// The §8.2 rules. For collections the base is `ext_base`.
    fn capture(
        &mut self,
        ns: &str,
        id: &str,
        local: Option<&LocalRecord>,
        origin: Origin,
        initial_stamp_ms: Option<i64>,
    ) -> anyhow::Result<CaptureResult>;
    /// The effective merged record, without local fields.
    fn merged(&self, ns: &str, id: &str) -> anyhow::Result<Option<LocalRecord>>;
    /// The feed, `ext_base`, `ext_view` and `reads` tables.
    fn store(&mut self) -> &mut dyn ExtStore;
    fn keys(&self) -> &Keys;
    fn device(&self) -> DeviceId;
}

/// Where the host sources find their files.
pub struct HostDirs {
    /// `<config>/kage`: `config.json`'s folder, where the steering docs
    /// live.
    pub config_kage_dir: PathBuf,
    /// `<data_local>/kage/sync`: the engine's own folder; chat mirrors go
    /// in `chats/` there.
    pub sync_dir: PathBuf,
}

/// The sources WP-S3 owns, for the app to hand the engine: single
/// settings, lists, steering docs and, when `chats` is given, chat
/// history.
pub fn host_sources(
    port: Arc<dyn ConfigPort>,
    dirs: HostDirs,
    os: super::types::OsKind,
    chats: Option<Arc<dyn chats::ChatHistory>>,
) -> Vec<Box<dyn Source>> {
    let mut out: Vec<Box<dyn Source>> = vec![
        Box::new(config::ConfigSource::new(port.clone())),
        Box::new(lists::ListsSource::new(port.clone(), os)),
        Box::new(steering::SteeringSource::new(
            port,
            dirs.config_kage_dir.clone(),
        )),
    ];
    if let Some(history) = chats {
        out.push(Box::new(chats::ChatsSource::new(
            history,
            dirs.sync_dir.clone(),
        )));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unchanged_marker_is_recognised_only_alone() {
        assert!(is_unchanged(&unchanged()));
        assert!(!is_unchanged(
            &serde_json::json!({ "$unchanged": true, "x": 1 })
        ));
        assert!(!is_unchanged(&serde_json::json!({ "$unchanged": false })));
        assert!(!is_unchanged(&serde_json::json!("x")));
    }
}

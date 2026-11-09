//! Sources: the parts of Kage that sync plugs into (docs/design/SYNC.md
//! §19.4). Each reads its local files as records and writes merged records
//! back. WP-S3 writes `config`, `lists`, `steering` and `chats`; WP-S5 the
//! extension source.
//!
//! C0 skeleton: the trait and its types. No source exists yet.

pub mod chats;
pub mod config;
pub mod lists;
pub mod steering;

use super::crypto::Keys;
use super::replica::ExtStore;
use super::types::{Category, DefaultScope, DeviceId, Origin, Structure};
use super::{ExtReply, ExtRequest};
use crate::error::AppError;
use std::collections::BTreeMap;

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

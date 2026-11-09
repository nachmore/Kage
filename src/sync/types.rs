//! The data model of folder sync (docs/design/SYNC.md §7.2) and the small
//! enums every part of the engine shares.
//!
//! C0 skeleton: types only. WP-S1 adds the text forms (`Stamp` as
//! `<16 hex><16 base32>`, `FieldKey` as `name@scope`) and their tests.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Version of the folder layout this build writes (`kage-sync.json`).
pub const SYNC_FORMAT: u32 = 1;
/// Oldest layout this build can read. A head or descriptor whose
/// `min_reader` is above it means "update Kage" (§8.11).
pub const READER_VERSION: u32 = 1;

/// One computer: 80 random bits, 16 lowercase Crockford base32 characters
/// on disk. Bytes order the same way as their text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DeviceId(pub [u8; 10]);

/// A hybrid logical clock value and the computer that issued it. Ordered by
/// `(hlc, dev)`, so two stamps are never equal unless they are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Stamp {
    pub hlc: u64,
    pub dev: DeviceId,
}

/// The OS a computer runs. Per-OS cells and list `avail` use these names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsKind {
    Macos,
    Windows,
    Linux,
}

/// Where a cell applies: every computer, or one OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    All,
    Os(OsKind),
}

/// One cell's name inside a record: a field (or set element, map entry,
/// group) at one scope. Text form `name@all`, `shortcut@macos`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldKey {
    pub name: String,
    pub scope: Scope,
}

/// One field of one record at one scope. `anc` holds at most 3 stamps: the
/// values this cell knowingly replaced (§7.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub value: serde_json::Value,
    pub stamp: Stamp,
    pub wall_ms: i64,
    pub anc: Vec<Stamp>,
}

/// A delete. `anc` holds the newest cell stamps the deleter saw, at most 3.
#[derive(Debug, Clone, PartialEq)]
pub struct Tomb {
    pub stamp: Stamp,
    pub wall_ms: i64,
    pub anc: Vec<Stamp>,
}

/// One record of a namespace (§7.1): its cells and, maybe, a tombstone.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Record {
    pub ns: String,
    pub id: String,
    pub cells: BTreeMap<FieldKey, Cell>,
    pub del: Option<Tomb>,
}

/// The categories of Settings → Sync (§4.1). `System` is sync's own
/// bookkeeping, which is always on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Settings,
    Automations,
    Extensions,
    Chats,
    System,
}

/// Why a local change happened. Capture and the guards treat them
/// differently (§8.2, §8.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    User,
    Import,
    Reset,
    Join,
    Sync,
}

/// The scope a new value of a catalog row goes to by default (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefaultScope {
    Shared,
    PerOs,
}

/// `sync_set_scope` actions (§9.3). The `avail_*` ones apply to list items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeAction {
    All,
    Os,
    UseShared,
    ThisComputer,
    StopThisComputer,
    AvailAll,
    AvailOs,
    AvailThisComputer,
}

/// What a conflicts-list entry records (§10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    ConcurrentEdit,
    EditVsDelete,
    DeleteVsEdit,
    ReplacedOnJoin,
    RestoreBackup,
    ReplacedByRestore,
    DeletedWhileAway,
}

/// One side of a conflict: the losing or the winning version.
#[derive(Debug, Clone, PartialEq)]
pub struct Side {
    pub stamp: Stamp,
    pub wall_ms: i64,
    pub deleted: bool,
    pub value: Option<serde_json::Value>,
}

/// The `body@all` cell of a `sync.conflicts` record (§10.1). It never
/// changes once written.
#[derive(Debug, Clone, PartialEq)]
pub struct ConflictBody {
    pub ns: String,
    pub rid: String,
    pub field: Option<FieldKey>,
    pub kind: ConflictKind,
    pub loser: Side,
    pub winner: Option<Side>,
}

/// Why a value on this computer is pinned instead of following the synced
/// cells (`pins.reason`, §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinReason {
    ThisComputer,
    HeldBack,
    Lossy,
    Consent,
    NeedsApproval,
    InvalidHere,
    NotInstalled,
    NewerSchema,
    OriginMismatch,
}

/// A local value's address in the `projected` and `pins` tables.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalKey {
    pub ns: String,
    pub id: String,
    pub field: String,
}

/// How a namespace's record fields become cells (§7.2): the whole record
/// as one cell, groups that change together, sets, maps, fields that stay
/// local, and fields that default to per OS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Structure {
    pub whole: bool,
    pub together: Vec<Vec<String>>,
    pub sets: Vec<String>,
    pub maps: Vec<String>,
    pub local: Vec<String>,
    pub per_os: Vec<String>,
}

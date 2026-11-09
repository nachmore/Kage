//! The data model of folder sync (docs/design/SYNC.md §7.2) and the small
//! enums every part of the engine shares.
//!
//! Text forms, used in shards, heads and the replica:
//! - [`DeviceId`]: 16 lowercase Crockford base32 characters (80 bits).
//! - [`Stamp`]: `<16 lowercase hex of the HLC><device id>`, 32 characters.
//!   The text sorts the same way as the stamp.
//! - [`Scope`]: `all`, `macos`, `windows`, `linux`.
//! - [`FieldKey`]: `name@scope`. The scope is after the *last* `@`, so a
//!   set element or map key may contain `@`.
//! - [`Cell`]: `[value, stamp, wall_ms]`, plus a fourth item with the
//!   ancestor stamps when there are any (§5.6).
//! - [`Tomb`]: `[stamp, wall_ms, ancestors]`.
//! - [`Record`]: `{ "id", "c": { field key: cell }, "x": tomb }` without
//!   its namespace, which the shard names once ([`Record::to_json`]).

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// Version of the folder layout this build writes (`kage-sync.json`).
pub const SYNC_FORMAT: u32 = 1;
/// Oldest layout this build can read. A head or descriptor whose
/// `min_reader` is above it means "update Kage" (§8.11).
pub const READER_VERSION: u32 = 1;

/// Crockford base32, lowercase. ASCII order matches digit order, so the
/// text of a device id sorts like its bytes.
const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A text form that didn't parse. Holds what was expected, never the input
/// (it may come from a file we don't trust, and logs stay short).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError(pub &'static str);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a valid {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// One computer: 80 random bits, 16 lowercase Crockford base32 characters
/// on disk. Bytes order the same way as their text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DeviceId(pub [u8; 10]);

impl DeviceId {
    /// Characters in the text form.
    pub const TEXT_LEN: usize = 16;

    /// A new random id from the OS random generator.
    pub fn random() -> Self {
        use rand::RngCore;
        let mut b = [0u8; 10];
        rand::rng().fill_bytes(&mut b);
        Self(b)
    }

    fn bits(&self) -> u128 {
        self.0
            .iter()
            .fold(0u128, |acc, b| (acc << 8) | u128::from(*b))
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.bits();
        let mut out = [0u8; Self::TEXT_LEN];
        for (i, c) in out.iter_mut().enumerate() {
            let shift = 75 - 5 * i as u32;
            *c = CROCKFORD[((v >> shift) & 31) as usize];
        }
        // Every byte comes from CROCKFORD, which is ASCII.
        f.write_str(std::str::from_utf8(&out).unwrap_or_default())
    }
}

impl FromStr for DeviceId {
    type Err = ParseError;

    /// Only the canonical lowercase form: R7 wants one spelling per name.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const ERR: ParseError = ParseError("device id");
        if s.len() != Self::TEXT_LEN {
            return Err(ERR);
        }
        let mut v = 0u128;
        for c in s.bytes() {
            let d = CROCKFORD.iter().position(|x| *x == c).ok_or(ERR)?;
            v = (v << 5) | d as u128;
        }
        let mut b = [0u8; 10];
        for (i, byte) in b.iter_mut().enumerate() {
            *byte = (v >> (72 - 8 * i as u32)) as u8;
        }
        Ok(Self(b))
    }
}

/// A hybrid logical clock value and the computer that issued it. Ordered by
/// `(hlc, dev)`, so two stamps are never equal unless they are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Stamp {
    pub hlc: u64,
    pub dev: DeviceId,
}

impl Stamp {
    /// Characters in the text form.
    pub const TEXT_LEN: usize = 16 + DeviceId::TEXT_LEN;

    pub fn new(hlc: u64, dev: DeviceId) -> Self {
        Self { hlc, dev }
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}{}", self.hlc, self.dev)
    }
}

impl FromStr for Stamp {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const ERR: ParseError = ParseError("stamp");
        if s.len() != Self::TEXT_LEN || !s.is_ascii() {
            return Err(ERR);
        }
        let (hex, dev) = s.split_at(16);
        if !hex
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(ERR);
        }
        let hlc = u64::from_str_radix(hex, 16).map_err(|_| ERR)?;
        Ok(Self {
            hlc,
            dev: dev.parse().map_err(|_| ERR)?,
        })
    }
}

/// The OS a computer runs. Per-OS cells and list `avail` use these names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsKind {
    Macos,
    Windows,
    Linux,
}

impl OsKind {
    /// Every OS, in the order scopes sort.
    pub const ALL: [OsKind; 3] = [OsKind::Macos, OsKind::Windows, OsKind::Linux];

    /// The wire name: `macos`, `windows` or `linux`.
    pub fn as_str(self) -> &'static str {
        match self {
            OsKind::Macos => "macos",
            OsKind::Windows => "windows",
            OsKind::Linux => "linux",
        }
    }

    /// The OS for a wire name. `None` for a name this build doesn't know.
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == s)
    }
}

/// Where a cell applies: every computer, or one OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    All,
    Os(OsKind),
}

impl Scope {
    /// The wire name: `all` or an OS name.
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::All => "all",
            Scope::Os(os) => os.as_str(),
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Scope {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "all" {
            return Ok(Scope::All);
        }
        OsKind::from_name(s)
            .map(Scope::Os)
            .ok_or(ParseError("scope"))
    }
}

/// One cell's name inside a record: a field (or set element, map entry,
/// group) at one scope. Text form `name@all`, `shortcut@macos`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldKey {
    pub name: String,
    pub scope: Scope,
}

impl FieldKey {
    pub fn new(name: impl Into<String>, scope: Scope) -> Self {
        Self {
            name: name.into(),
            scope,
        }
    }

    /// `name@all`.
    pub fn all(name: impl Into<String>) -> Self {
        Self::new(name, Scope::All)
    }

    /// `name@<os>`.
    pub fn os(name: impl Into<String>, os: OsKind) -> Self {
        Self::new(name, Scope::Os(os))
    }
}

impl fmt::Display for FieldKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.scope)
    }
}

impl FromStr for FieldKey {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const ERR: ParseError = ParseError("field key");
        let (name, scope) = s.rsplit_once('@').ok_or(ERR)?;
        if name.is_empty() {
            return Err(ERR);
        }
        Ok(Self {
            name: name.to_string(),
            scope: scope.parse().map_err(|_| ERR)?,
        })
    }
}

/// Serde through the text form, for the types above.
macro_rules! serde_as_text {
    ($t:ty, $what:literal) => {
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl Visitor<'_> for V {
                    type Value = $t;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str($what)
                    }
                    fn visit_str<E: de::Error>(self, v: &str) -> Result<$t, E> {
                        v.parse().map_err(E::custom)
                    }
                }
                d.deserialize_str(V)
            }
        }
    };
}

serde_as_text!(DeviceId, "a device id");
serde_as_text!(Stamp, "a stamp");
serde_as_text!(Scope, "a scope");
serde_as_text!(FieldKey, "a field key");

/// One field of one record at one scope. `anc` holds at most 3 stamps: the
/// values this cell knowingly replaced (§7.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub value: serde_json::Value,
    pub stamp: Stamp,
    pub wall_ms: i64,
    pub anc: Vec<Stamp>,
}

impl Serialize for Cell {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let len = if self.anc.is_empty() { 3 } else { 4 };
        let mut seq = s.serialize_seq(Some(len))?;
        seq.serialize_element(&self.value)?;
        seq.serialize_element(&self.stamp)?;
        seq.serialize_element(&self.wall_ms)?;
        if !self.anc.is_empty() {
            seq.serialize_element(&self.anc)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Cell;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a cell [value, stamp, wall, ancestors?]")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Cell, A::Error> {
                let value = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let stamp = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let wall_ms = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                let anc = seq.next_element()?.unwrap_or_default();
                // A newer build may append items; they are ignored.
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(Cell {
                    value,
                    stamp,
                    wall_ms,
                    anc,
                })
            }
        }
        d.deserialize_seq(V)
    }
}

/// A delete. `anc` holds the newest cell stamps the deleter saw, at most 3.
#[derive(Debug, Clone, PartialEq)]
pub struct Tomb {
    pub stamp: Stamp,
    pub wall_ms: i64,
    pub anc: Vec<Stamp>,
}

impl Serialize for Tomb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (&self.stamp, self.wall_ms, &self.anc).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Tomb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Tomb;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a tombstone [stamp, wall, ancestors]")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Tomb, A::Error> {
                let stamp = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let wall_ms = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let anc = seq.next_element()?.unwrap_or_default();
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(Tomb {
                    stamp,
                    wall_ms,
                    anc,
                })
            }
        }
        d.deserialize_seq(V)
    }
}

/// One record of a namespace (§7.1): its cells and, maybe, a tombstone.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Record {
    pub ns: String,
    pub id: String,
    pub cells: BTreeMap<FieldKey, Cell>,
    pub del: Option<Tomb>,
}

impl Record {
    /// An empty record: no cells, no tombstone.
    pub fn new(ns: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            ns: ns.into(),
            id: id.into(),
            ..Self::default()
        }
    }

    /// The cell `name@scope`, if there is one.
    pub fn cell(&self, name: &str, scope: Scope) -> Option<&Cell> {
        self.cells.get(&FieldKey::new(name, scope))
    }

    /// The highest stamp in the record: every cell and the tombstone.
    pub fn max_stamp(&self) -> Option<Stamp> {
        self.cells
            .values()
            .map(|c| c.stamp)
            .chain(self.del.as_ref().map(|d| d.stamp))
            .max()
    }

    /// The newest `n` cell stamps, newest first: the `anc` of a delete
    /// made by someone who sees this record (§8.2).
    pub fn newest_cell_stamps(&self, n: usize) -> Vec<Stamp> {
        let mut s: Vec<Stamp> = self.cells.values().map(|c| c.stamp).collect();
        s.sort_unstable_by(|a, b| b.cmp(a));
        s.dedup();
        s.truncate(n);
        s
    }

    /// The shard form of §5.6: `{ "id", "c", "x"? }`. The namespace isn't
    /// in it; the shard names it once.
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("id".into(), serde_json::Value::String(self.id.clone()));
        let cells = self
            .cells
            .iter()
            .map(|(k, c)| (k.to_string(), serde_json::to_value(c).unwrap_or_default()))
            .collect();
        m.insert("c".into(), serde_json::Value::Object(cells));
        if let Some(d) = &self.del {
            m.insert("x".into(), serde_json::to_value(d).unwrap_or_default());
        }
        serde_json::Value::Object(m)
    }

    /// Read the shard form back. A cell or tombstone that doesn't parse
    /// fails the whole record: a sealed file is never half-trusted.
    pub fn from_json(ns: &str, v: &serde_json::Value) -> anyhow::Result<Self> {
        let obj = v
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("record is not an object"))?;
        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("record has no id"))?;
        let mut cells = BTreeMap::new();
        if let Some(c) = obj.get("c") {
            let c = c
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("record cells are not an object"))?;
            for (k, v) in c {
                cells.insert(k.parse()?, Cell::deserialize(v)?);
            }
        }
        let del = match obj.get("x") {
            None | Some(serde_json::Value::Null) => None,
            Some(x) => Some(Tomb::deserialize(x)?),
        };
        Ok(Self {
            ns: ns.to_string(),
            id: id.to_string(),
            cells,
            del,
        })
    }
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
///
/// JSON: `{ "stamp", "wall", "deleted", "value"? }`. `value` is left out
/// when there is none (a delete, or a winner, whose value the record
/// itself holds); a present `null` is a real value ("no value at this
/// scope").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Side {
    pub stamp: Stamp,
    #[serde(rename = "wall")]
    pub wall_ms: i64,
    #[serde(default)]
    pub deleted: bool,
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<serde_json::Value>,
}

/// A present JSON `null` is `Some(Null)`; only a missing key is `None`.
fn present_value<'de, D: Deserializer<'de>>(d: D) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(d).map(Some)
}

/// The `body@all` cell of a `sync.conflicts` record (§10.1). It never
/// changes once written.
///
/// JSON: `{ "ns", "rid", "field": "name@scope" | null, "kind", "loser",
/// "winner": side | null }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl PinReason {
    const ALL: [PinReason; 9] = [
        PinReason::ThisComputer,
        PinReason::HeldBack,
        PinReason::Lossy,
        PinReason::Consent,
        PinReason::NeedsApproval,
        PinReason::InvalidHere,
        PinReason::NotInstalled,
        PinReason::NewerSchema,
        PinReason::OriginMismatch,
    ];

    /// The wire name, as stored in `pins.reason`.
    pub fn as_str(self) -> &'static str {
        match self {
            PinReason::ThisComputer => "this_computer",
            PinReason::HeldBack => "held_back",
            PinReason::Lossy => "lossy",
            PinReason::Consent => "consent",
            PinReason::NeedsApproval => "needs_approval",
            PinReason::InvalidHere => "invalid_here",
            PinReason::NotInstalled => "not_installed",
            PinReason::NewerSchema => "newer_schema",
            PinReason::OriginMismatch => "origin_mismatch",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

/// A local value's address in the `projected` and `pins` tables.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalKey {
    pub ns: String,
    pub id: String,
    pub field: String,
}

impl LocalKey {
    pub fn new(ns: impl Into<String>, id: impl Into<String>, field: impl Into<String>) -> Self {
        Self {
            ns: ns.into(),
            id: id.into(),
            field: field.into(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dev(n: u8) -> DeviceId {
        DeviceId([n, 1, 2, 3, 4, 5, 6, 7, 8, n])
    }

    fn cmp_stamp_text(a: &Stamp, b: &Stamp) -> std::cmp::Ordering {
        a.to_string().cmp(&b.to_string())
    }

    #[test]
    fn device_ids_round_trip_and_sort_like_their_text() {
        for _ in 0..200 {
            let a = DeviceId::random();
            let b = DeviceId::random();
            let (ta, tb) = (a.to_string(), b.to_string());
            assert_eq!(ta.len(), 16);
            assert!(ta.bytes().all(|c| CROCKFORD.contains(&c)));
            assert_eq!(ta.parse::<DeviceId>().unwrap(), a);
            assert_eq!(a.cmp(&b), ta.cmp(&tb));
        }
        assert_eq!(DeviceId([0; 10]).to_string(), "0000000000000000");
        assert_eq!(DeviceId([0xff; 10]).to_string(), "zzzzzzzzzzzzzzzz");
        // Only the canonical spelling.
        for bad in [
            "",
            "0000000000000000z",
            "000000000000000I",
            "000000000000000U",
        ] {
            assert!(bad.parse::<DeviceId>().is_err(), "{bad}");
        }
        assert!("ABCDEFGHJKMNPQRS".parse::<DeviceId>().is_err());
    }

    #[test]
    fn stamps_round_trip_and_sort_like_their_text() {
        let s = Stamp::new(0x0001_930b_6f1c_0004, "k7m2p9xqa3rt5vwe".parse().unwrap());
        assert_eq!(s.to_string(), "0001930b6f1c0004k7m2p9xqa3rt5vwe");
        assert_eq!(s.to_string().parse::<Stamp>().unwrap(), s);
        let mut stamps = vec![
            Stamp::new(5, dev(1)),
            Stamp::new(5, dev(0)),
            Stamp::new(u64::MAX, dev(0)),
            Stamp::new(0, dev(9)),
            Stamp::new(1 << 40, dev(3)),
        ];
        for _ in 0..50 {
            stamps.push(Stamp::new(rand::random(), DeviceId::random()));
        }
        for a in &stamps {
            for b in &stamps {
                assert_eq!(a.cmp(b), cmp_stamp_text(a, b));
            }
        }
        for bad in [
            "",
            "0001930B6f1c0004k7m2p9xqa3rt5vwe",
            "0001930b6f1c000k7m2p9xqa3rt5vwe",
            "+001930b6f1c0004k7m2p9xqa3rt5vwe",
            "0001930b6f1c0004k7m2p9xqa3rt5vwé",
        ] {
            assert!(bad.parse::<Stamp>().is_err(), "{bad}");
        }
    }

    #[test]
    fn field_keys_split_at_the_last_at_sign() {
        let k: FieldKey = "shortcut@macos".parse().unwrap();
        assert_eq!(k, FieldKey::os("shortcut", OsKind::Macos));
        let k: FieldKey = "tags[]\"a@b\"@all".parse().unwrap();
        assert_eq!(k.name, "tags[]\"a@b\"");
        assert_eq!(k.scope, Scope::All);
        assert_eq!(k.to_string(), "tags[]\"a@b\"@all");
        for bad in ["name", "@all", "name@mac", "name@"] {
            assert!(bad.parse::<FieldKey>().is_err(), "{bad}");
        }
        for s in ["all", "macos", "windows", "linux"] {
            assert_eq!(s.parse::<Scope>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn cells_and_tombstones_use_the_shard_form() {
        let a = Stamp::new(0x0001_930b_6f1c_0004, dev(1));
        let b = Stamp::new(0x0001_930a_1122_0000, dev(2));
        let plain = Cell {
            value: json!("Open notes"),
            stamp: a,
            wall_ms: 1_762_686_000_000,
            anc: vec![],
        };
        let with_anc = Cell {
            anc: vec![b],
            ..plain.clone()
        };
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            json!(["Open notes", a.to_string(), 1_762_686_000_000_i64])
        );
        assert_eq!(
            serde_json::to_value(&with_anc).unwrap(),
            json!([
                "Open notes",
                a.to_string(),
                1_762_686_000_000_i64,
                [b.to_string()]
            ])
        );
        for c in [&plain, &with_anc] {
            let back: Cell = serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap();
            assert_eq!(&back, c);
        }
        // A newer build's extra items are ignored.
        let extra: Cell =
            serde_json::from_value(json!([1, a.to_string(), 5, [], "future"])).unwrap();
        assert_eq!(extra.value, json!(1));
        assert!(serde_json::from_value::<Cell>(json!([1, a.to_string()])).is_err());

        let t = Tomb {
            stamp: a,
            wall_ms: 7,
            anc: vec![],
        };
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            json!([a.to_string(), 7, []])
        );
        let back: Tomb = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn records_round_trip_through_the_shard_form() {
        let a = Stamp::new(10, dev(1));
        let mut r = Record::new("list.shortcuts", "s7hq2m0c5d1z9w3pa");
        r.cells.insert(
            FieldKey::all("name"),
            Cell {
                value: json!("Open notes"),
                stamp: a,
                wall_ms: 1,
                anc: vec![],
            },
        );
        r.cells.insert(
            FieldKey::os("shortcut", OsKind::Windows),
            Cell {
                value: json!(null),
                stamp: Stamp::new(11, dev(2)),
                wall_ms: 2,
                anc: vec![a],
            },
        );
        let j = r.to_json();
        assert_eq!(j["id"], "s7hq2m0c5d1z9w3pa");
        assert!(j["c"].get("shortcut@windows").is_some());
        assert!(j.get("x").is_none());
        assert_eq!(Record::from_json("list.shortcuts", &j).unwrap(), r);

        r.del = Some(Tomb {
            stamp: Stamp::new(12, dev(1)),
            wall_ms: 3,
            anc: vec![Stamp::new(11, dev(2)), a],
        });
        let back = Record::from_json("list.shortcuts", &r.to_json()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.max_stamp(), Some(Stamp::new(12, dev(1))));
        assert_eq!(back.newest_cell_stamps(3), vec![Stamp::new(11, dev(2)), a]);

        assert!(Record::from_json("x", &json!({ "c": {} })).is_err());
        assert!(
            Record::from_json("x", &json!({ "id": "a", "c": { "bad": [1, "x", 1] } })).is_err()
        );
    }

    #[test]
    fn conflict_bodies_use_the_documented_json() {
        let lo = Stamp::new(0x0001_9308_0b0e_0001, dev(2));
        let w = Stamp::new(0x0001_930b_6f1c_0004, dev(1));
        let body = ConflictBody {
            ns: "list.shortcuts".into(),
            rid: "s7hq".into(),
            field: Some(FieldKey::all("prompt")),
            kind: ConflictKind::ConcurrentEdit,
            loser: Side {
                stamp: lo,
                wall_ms: 1_762_640_000_000,
                deleted: false,
                value: Some(json!("Summarize: {text}")),
            },
            winner: Some(Side {
                stamp: w,
                wall_ms: 1_762_686_000_000,
                deleted: false,
                value: None,
            }),
        };
        let j = serde_json::to_value(&body).unwrap();
        assert_eq!(
            j,
            json!({
                "ns": "list.shortcuts", "rid": "s7hq", "field": "prompt@all",
                "kind": "concurrent_edit",
                "loser": { "stamp": lo.to_string(), "wall": 1_762_640_000_000_i64,
                           "deleted": false, "value": "Summarize: {text}" },
                "winner": { "stamp": w.to_string(), "wall": 1_762_686_000_000_i64, "deleted": false }
            })
        );
        assert_eq!(serde_json::from_value::<ConflictBody>(j).unwrap(), body);

        // A `null` loser value survives; a missing one is `None`.
        let mut nulled = body.clone();
        nulled.loser.value = Some(serde_json::Value::Null);
        nulled.field = None;
        nulled.winner = None;
        let j = serde_json::to_value(&nulled).unwrap();
        assert_eq!(j["loser"]["value"], serde_json::Value::Null);
        assert!(j["loser"].as_object().unwrap().contains_key("value"));
        assert_eq!(serde_json::from_value::<ConflictBody>(j).unwrap(), nulled);
    }

    #[test]
    fn pin_reasons_and_os_names_round_trip() {
        for r in PinReason::ALL {
            assert_eq!(PinReason::from_name(r.as_str()), Some(r));
            assert_eq!(serde_json::to_value(r).unwrap(), json!(r.as_str()));
        }
        for o in OsKind::ALL {
            assert_eq!(OsKind::from_name(o.as_str()), Some(o));
            assert_eq!(serde_json::to_value(o).unwrap(), json!(o.as_str()));
        }
        assert_eq!(OsKind::from_name("android"), None);
    }
}

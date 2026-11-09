//! This computer's replica (`<data_local>/kage/sync/replica.db`,
//! docs/design/SYNC.md §6.2): every record it has merged, what each local
//! file shows, pins, devices, and the extension tables. The source of truth
//! for sync; `config.json` and extension data are projections of it.
//!
//! - SQLite in WAL mode, one connection, owned by the engine thread.
//! - The schema is versioned with `PRAGMA user_version` and migrated
//!   forward on open, one step at a time ([`MIGRATIONS`]).
//! - Every change goes through [`Replica::tx`]: one transaction, committed
//!   only when the closure returns `Ok`. An error or a panic rolls it back,
//!   so a crash never leaves half a shard merged.
//! - A file that isn't a readable replica of this or an older schema is
//!   moved aside (never deleted) and a new one is made:
//!   [`OpenNote::RebuiltFromCorrupt`]. The engine then rebuilds it from the
//!   folder (§15).
//!
//! Buckets: `records.bucket` holds a record's 16-bit bucket hash,
//! `Keys::bucket(ns, id, 1 << 16)`. Any power-of-two bucket count up to
//! 65,536 is that hash modulo the count, so [`Tx::records_in_bucket`]
//! works for whatever count a namespace uses now.
//!
//! Values are stored as serde_json writes them, so a value reads back
//! exactly as it was merged. Stamps, scopes and device ids use their text
//! forms; an `anc` list is its stamps joined by `,`.

use super::merge::{merge_record, MergeOutcome};
use super::types::{
    Cell, DeviceId, FieldKey, LocalKey, OsKind, PinReason, Record, Scope, Stamp, Tomb,
};
use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How [`Replica::open`] found the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenNote {
    /// There was no file; a new one was created.
    Fresh,
    /// An existing replica opened normally.
    Existing,
    /// The file was corrupt. It was moved aside and a new one created; the
    /// engine rebuilds it from the folder (§15).
    RebuiltFromCorrupt,
}

/// What a local file shows for one field, and which cell it came from:
/// the value's hash, the cell's stamp and scope (`None` for a local value).
pub type Projected = ([u8; 32], Option<Stamp>, Option<Scope>);

/// One row of the `devices` table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceRow {
    pub dev: DeviceId,
    pub name: String,
    pub os: Option<OsKind>,
    pub kage_version: String,
    pub head_seq: u64,
    pub ingested_seq: u64,
    pub last_seen_ms: i64,
    pub instance: String,
    pub min_reader: u32,
    pub gc_floor: Option<Stamp>,
    /// `ok`, `stale`, `inactive`, `needs_update` or `retired`.
    pub state: String,
}

/// The schema version this build writes.
pub const SCHEMA_VERSION: u32 = 1;

/// `MIGRATIONS[n]` takes the schema from version `n` to `n + 1`. Append
/// only; never edit a step that has shipped.
const MIGRATIONS: &[&str] = &[
    // 0 → 1: the tables of §6.2.
    "
    CREATE TABLE meta      (k TEXT PRIMARY KEY, v TEXT NOT NULL) WITHOUT ROWID;
    CREATE TABLE cells     (ns TEXT NOT NULL, id TEXT NOT NULL, field TEXT NOT NULL, scope TEXT NOT NULL,
                            value BLOB NOT NULL, stamp TEXT NOT NULL, wall INTEGER NOT NULL,
                            anc TEXT NOT NULL DEFAULT '',
                            PRIMARY KEY (ns, id, field, scope)) WITHOUT ROWID;
    CREATE TABLE records   (ns TEXT NOT NULL, id TEXT NOT NULL, bucket INTEGER NOT NULL,
                            del_stamp TEXT, del_wall INTEGER, del_anc TEXT,
                            published INTEGER NOT NULL DEFAULT 0,
                            PRIMARY KEY (ns, id)) WITHOUT ROWID;
    CREATE INDEX records_by_bucket ON records (ns, bucket);
    CREATE TABLE projected (ns TEXT NOT NULL, id TEXT NOT NULL, field TEXT NOT NULL,
                            hash BLOB NOT NULL, stamp TEXT, scope TEXT,
                            PRIMARY KEY (ns, id, field)) WITHOUT ROWID;
    CREATE TABLE pins      (ns TEXT NOT NULL, id TEXT NOT NULL, field TEXT NOT NULL,
                            value BLOB NOT NULL, reason TEXT NOT NULL,
                            PRIMARY KEY (ns, id, field)) WITHOUT ROWID;
    CREATE TABLE devices   (dev TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '', os TEXT,
                            kage_version TEXT NOT NULL DEFAULT '', head_seq INTEGER NOT NULL DEFAULT 0,
                            ingested_seq INTEGER NOT NULL DEFAULT 0, last_seen INTEGER NOT NULL DEFAULT 0,
                            instance TEXT NOT NULL DEFAULT '', min_reader INTEGER NOT NULL DEFAULT 0,
                            gc_floor TEXT, state TEXT NOT NULL DEFAULT '') WITHOUT ROWID;
    CREATE TABLE applied   (name TEXT PRIMARY KEY) WITHOUT ROWID;
    CREATE TABLE acks      (ns TEXT NOT NULL, id TEXT NOT NULL, dev TEXT NOT NULL,
                            PRIMARY KEY (ns, id, dev)) WITHOUT ROWID;
    CREATE TABLE dirty     (ns TEXT NOT NULL, bucket INTEGER NOT NULL, own INTEGER NOT NULL,
                            PRIMARY KEY (ns, bucket)) WITHOUT ROWID;
    CREATE TABLE feed      (ext TEXT NOT NULL, seq INTEGER NOT NULL, coll TEXT NOT NULL, id TEXT NOT NULL,
                            PRIMARY KEY (ext, seq)) WITHOUT ROWID;
    CREATE UNIQUE INDEX feed_by_item ON feed (ext, coll, id);
    CREATE TABLE ext_base  (ext TEXT NOT NULL, coll TEXT NOT NULL, id TEXT NOT NULL, field TEXT NOT NULL,
                            stamp TEXT NOT NULL, hash BLOB NOT NULL,
                            PRIMARY KEY (ext, coll, id, field)) WITHOUT ROWID;
    CREATE TABLE reads     (win TEXT NOT NULL, ext TEXT NOT NULL, key TEXT NOT NULL, stamp TEXT NOT NULL,
                            PRIMARY KEY (win, ext, key)) WITHOUT ROWID;
    CREATE TABLE ext_view  (win TEXT NOT NULL, ext TEXT NOT NULL, key TEXT NOT NULL, ids BLOB NOT NULL,
                            PRIMARY KEY (win, ext, key)) WITHOUT ROWID;
    ",
];

/// The replica database. Only the engine thread holds it.
pub struct Replica {
    conn: Connection,
}

impl Replica {
    /// Open (or create) the replica at `path`. The parent directory is
    /// created; a new file is owner-only.
    pub fn open(path: &Path) -> anyhow::Result<(Self, OpenNote)> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| "creating the sync state directory".to_string())?;
        }
        if !path.exists() {
            // SQLite gives its -wal and -shm files the database's
            // permissions, so this keeps all three owner-only on Unix.
            crate::atomic_write::write(path, b"", crate::atomic_write::Perms::Private)
                .context("creating replica.db")?;
        }
        match Self::open_checked(path) {
            Ok((conn, from)) => {
                let note = if from == 0 {
                    OpenNote::Fresh
                } else {
                    OpenNote::Existing
                };
                Ok((Self { conn }, note))
            }
            // A busy, unreadable or full disk is reported, never "fixed" by
            // starting over.
            Err(e) if !is_unusable(&e) => Err(e),
            Err(e) => {
                log::warn!("sync: replica.db can't be used ({e:#}); moving it aside");
                let aside = move_aside(path)?;
                log::warn!(
                    "sync: moved the old replica to {}",
                    aside.file_name().unwrap_or_default().to_string_lossy()
                );
                crate::atomic_write::write(path, b"", crate::atomic_write::Perms::Private)
                    .context("creating replica.db")?;
                let (conn, _) = Self::open_checked(path)?;
                Ok((Self { conn }, OpenNote::RebuiltFromCorrupt))
            }
        }
    }

    /// A replica that lives in memory, for tests.
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Open, check and migrate. Returns the schema version found.
    fn open_checked(path: &Path) -> anyhow::Result<(Connection, u32)> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            log::warn!("sync: replica.db runs in {mode} journal mode, not WAL");
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(Unusable("integrity check failed").into());
        }
        let from = migrate(&conn)?;
        Ok((conn, from))
    }

    /// Run `f` in one transaction. Every multi-row change goes through here.
    pub fn tx<T>(&mut self, f: impl FnOnce(&mut Tx<'_>) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let t = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut tx = Tx { tx: t };
        let out = f(&mut tx)?;
        tx.tx.commit()?;
        Ok(out)
    }
}

/// A file at the replica's path that isn't a replica this build can use:
/// damaged, written by a newer build, or some other database.
#[derive(Debug, Clone, Copy)]
struct Unusable(&'static str);

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Unusable {}

/// Whether an open error means the file itself can't be used, as against
/// a passing problem (busy, no permission, disk full).
fn is_unusable(e: &anyhow::Error) -> bool {
    use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
    e.chain().any(|c| {
        c.is::<Unusable>()
            || matches!(
                c.downcast_ref::<rusqlite::Error>(),
                Some(rusqlite::Error::SqliteFailure(f, _))
                    if matches!(f.code, NotADatabase | DatabaseCorrupt)
            )
    })
}

/// Bring the schema up to [`SCHEMA_VERSION`]. Returns the version found.
fn migrate(conn: &Connection) -> anyhow::Result<u32> {
    let from: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if from > SCHEMA_VERSION {
        return Err(Unusable("written by a newer Kage").into());
    }
    if from == 0 {
        let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))?;
        if tables > 0 {
            return Err(Unusable("some other database").into());
        }
    }
    for (v, step) in MIGRATIONS.iter().enumerate().skip(from as usize) {
        let to = v + 1;
        conn.execute_batch(&format!(
            "BEGIN IMMEDIATE; {step} PRAGMA user_version = {to}; COMMIT;"
        ))
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK;");
            anyhow::Error::from(e).context(format!("replica migration to {to} failed"))
        })?;
    }
    Ok(from)
}

/// Move `path` and its WAL files to `<name>.corrupt-<n>` names. A stale
/// WAL must not be applied to the new database.
fn move_aside(path: &Path) -> anyhow::Result<PathBuf> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "replica.db".into());
    let with = |suffix: &str| path.with_file_name(format!("{name}{suffix}"));
    let n = (1u32..)
        .find(|n| !with(&format!(".corrupt-{n}")).exists())
        .unwrap_or(0);
    let aside = with(&format!(".corrupt-{n}"));
    std::fs::rename(path, &aside).context("moving the old replica aside")?;
    for side in ["-wal", "-shm"] {
        let from = with(side);
        if from.exists() {
            let _ = std::fs::rename(&from, with(&format!(".corrupt-{n}{side}")));
        }
    }
    Ok(aside)
}

/// One transaction on the replica.
pub struct Tx<'a> {
    tx: rusqlite::Transaction<'a>,
}

fn anc_text(anc: &[Stamp]) -> String {
    anc.iter()
        .map(Stamp::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_anc(s: &str) -> anyhow::Result<Vec<Stamp>> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    s.split(',')
        .map(|p| p.parse::<Stamp>().map_err(anyhow::Error::from))
        .collect()
}

fn parse_stamp(s: &str) -> anyhow::Result<Stamp> {
    Ok(s.parse::<Stamp>()?)
}

fn hash32(b: Vec<u8>) -> anyhow::Result<[u8; 32]> {
    b.try_into()
        .map_err(|_| anyhow::anyhow!("a stored hash isn't 32 bytes"))
}

fn tomb_from(
    stamp: Option<String>,
    wall: Option<i64>,
    anc: Option<String>,
) -> anyhow::Result<Option<Tomb>> {
    match stamp {
        None => Ok(None),
        Some(s) => Ok(Some(Tomb {
            stamp: parse_stamp(&s)?,
            wall_ms: wall.unwrap_or(0),
            anc: parse_anc(anc.as_deref().unwrap_or(""))?,
        })),
    }
}

/// Raw cell columns: field, scope, value, stamp, wall, anc.
type CellRow = (String, String, Vec<u8>, String, i64, String);

fn cell_from(row: CellRow) -> anyhow::Result<(FieldKey, Cell)> {
    let (field, scope, value, stamp, wall, anc) = row;
    Ok((
        FieldKey::new(field, scope.parse()?),
        Cell {
            value: serde_json::from_slice(&value)?,
            stamp: parse_stamp(&stamp)?,
            wall_ms: wall,
            anc: parse_anc(&anc)?,
        },
    ))
}

impl Tx<'_> {
    pub fn record(&self, ns: &str, id: &str) -> anyhow::Result<Option<Record>> {
        let head = self
            .tx
            .prepare_cached(
                "SELECT del_stamp, del_wall, del_anc FROM records WHERE ns = ?1 AND id = ?2",
            )?
            .query_row(params![ns, id], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .optional()?;
        let Some((ds, dw, da)) = head else {
            return Ok(None);
        };
        let mut rec = Record::new(ns, id);
        rec.del = tomb_from(ds, dw, da)?;
        let mut stmt = self.tx.prepare_cached(
            "SELECT field, scope, value, stamp, wall, anc FROM cells WHERE ns = ?1 AND id = ?2",
        )?;
        let rows = stmt.query_map(params![ns, id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?;
        for row in rows {
            let (k, c) = cell_from(row?)?;
            rec.cells.insert(k, c);
        }
        Ok(Some(rec))
    }

    /// Store a record with its 16-bit bucket hash (module docs). Replaces
    /// its cells and tombstone; keeps whether it was published.
    pub fn put_record(&mut self, r: &Record, bucket: u32) -> anyhow::Result<()> {
        let (ds, dw, da) = match &r.del {
            Some(d) => (
                Some(d.stamp.to_string()),
                Some(d.wall_ms),
                Some(anc_text(&d.anc)),
            ),
            None => (None, None, None),
        };
        self.tx
            .prepare_cached(
                "INSERT INTO records (ns, id, bucket, del_stamp, del_wall, del_anc) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (ns, id) DO UPDATE SET bucket = excluded.bucket, del_stamp = excluded.del_stamp,
                 del_wall = excluded.del_wall, del_anc = excluded.del_anc",
            )?
            .execute(params![r.ns, r.id, bucket, ds, dw, da])?;
        self.tx
            .prepare_cached("DELETE FROM cells WHERE ns = ?1 AND id = ?2")?
            .execute(params![r.ns, r.id])?;
        let mut ins = self.tx.prepare_cached(
            "INSERT INTO cells (ns, id, field, scope, value, stamp, wall, anc) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (k, c) in &r.cells {
            ins.execute(params![
                r.ns,
                r.id,
                k.name,
                k.scope.as_str(),
                serde_json::to_vec(&c.value)?,
                c.stamp.to_string(),
                c.wall_ms,
                anc_text(&c.anc),
            ])?;
        }
        Ok(())
    }

    /// The records of `ns` in `bucket` out of `of` (a power of two), sorted
    /// by id.
    pub fn records_in_bucket(&self, ns: &str, bucket: u32, of: u32) -> anyhow::Result<Vec<Record>> {
        anyhow::ensure!(
            of.is_power_of_two() && of <= 1 << 16,
            "bad bucket count {of}"
        );
        self.query_records(
            "WHERE r.ns = ?1 AND (r.bucket & ?2) = ?3",
            params![ns, of - 1, bucket],
        )
    }

    /// Every record of `ns`, sorted by id.
    pub fn records(&self, ns: &str) -> anyhow::Result<Vec<Record>> {
        self.query_records("WHERE r.ns = ?1", params![ns])
    }

    fn query_records(
        &self,
        filter: &str,
        p: &[&dyn rusqlite::ToSql],
    ) -> anyhow::Result<Vec<Record>> {
        let sql = format!(
            "SELECT r.ns, r.id, r.del_stamp, r.del_wall, r.del_anc,
                    c.field, c.scope, c.value, c.stamp, c.wall, c.anc
             FROM records r LEFT JOIN cells c ON c.ns = r.ns AND c.id = r.id
             {filter} ORDER BY r.id"
        );
        let mut stmt = self.tx.prepare_cached(&sql)?;
        let mut rows = stmt.query(p)?;
        let mut out: Vec<Record> = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(1)?;
            if out.last().is_none_or(|r| r.id != id) {
                let mut rec = Record::new(row.get::<_, String>(0)?, id);
                rec.del = tomb_from(row.get(2)?, row.get(3)?, row.get(4)?)?;
                out.push(rec);
            }
            let field: Option<String> = row.get(5)?;
            if let Some(field) = field {
                let (k, c) = cell_from((
                    field,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))?;
                if let Some(rec) = out.last_mut() {
                    rec.cells.insert(k, c);
                }
            }
        }
        Ok(out)
    }

    /// Every namespace that has records.
    pub fn namespaces(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self
            .tx
            .prepare_cached("SELECT DISTINCT ns FROM records ORDER BY ns")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// How many records `ns` holds, tombstoned ones included.
    pub fn count_records(&self, ns: &str) -> anyhow::Result<u64> {
        Ok(self
            .tx
            .prepare_cached("SELECT COUNT(*) FROM records WHERE ns = ?1")?
            .query_row(params![ns], |r| r.get(0))?)
    }

    /// The stored bucket hash of a record.
    pub fn bucket_of(&self, ns: &str, id: &str) -> anyhow::Result<Option<u32>> {
        Ok(self
            .tx
            .prepare_cached("SELECT bucket FROM records WHERE ns = ?1 AND id = ?2")?
            .query_row(params![ns, id], |r| r.get(0))
            .optional()?)
    }

    /// Drop a record, its cells and its acks (tombstone cleanup, §8.7).
    pub fn delete_record(&mut self, ns: &str, id: &str) -> anyhow::Result<()> {
        for sql in [
            "DELETE FROM cells WHERE ns = ?1 AND id = ?2",
            "DELETE FROM acks WHERE ns = ?1 AND id = ?2",
            "DELETE FROM records WHERE ns = ?1 AND id = ?2",
        ] {
            self.tx.prepare_cached(sql)?.execute(params![ns, id])?;
        }
        Ok(())
    }

    /// Merge `incoming` into the stored record (or an empty one), and store
    /// it when it changed. A change is marked dirty as an echo (`own =
    /// false`). Conflicts are returned for the caller to record in the same
    /// transaction.
    pub fn merge(&mut self, incoming: &Record, bucket: u32) -> anyhow::Result<MergeOutcome> {
        let mut local = self
            .record(&incoming.ns, &incoming.id)?
            .unwrap_or_else(|| Record::new(incoming.ns.clone(), incoming.id.clone()));
        let out = merge_record(&mut local, incoming);
        if out.changed {
            self.put_record(&local, bucket)?;
            self.mark_dirty(&incoming.ns, bucket, false)?;
        }
        Ok(out)
    }

    /// Store a record changed by this computer and mark its bucket dirty
    /// as an own edit.
    pub fn write(&mut self, r: &Record, bucket: u32) -> anyhow::Result<()> {
        self.put_record(r, bucket)?;
        self.mark_dirty(&r.ns, bucket, true)
    }

    /// After a publish: every record of `ns` is in the folder.
    pub fn mark_published(&mut self, ns: &str) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("UPDATE records SET published = 1 WHERE ns = ?1 AND published = 0")?
            .execute(params![ns])?;
        Ok(())
    }

    /// Whether the record was ever published from here.
    pub fn is_published(&self, ns: &str, id: &str) -> anyhow::Result<bool> {
        Ok(self
            .tx
            .prepare_cached("SELECT published FROM records WHERE ns = ?1 AND id = ?2")?
            .query_row(params![ns, id], |r| r.get::<_, i64>(0))
            .optional()?
            .is_some_and(|p| p != 0))
    }

    /// Mark a bucket for the next publish. `own` is a change made here, as
    /// against one only echoed. Once own, a bucket stays own until taken.
    pub fn mark_dirty(&mut self, ns: &str, bucket: u32, own: bool) -> anyhow::Result<()> {
        self.tx
            .prepare_cached(
                "INSERT INTO dirty (ns, bucket, own) VALUES (?1, ?2, ?3)
                 ON CONFLICT (ns, bucket) DO UPDATE SET own = MAX(own, excluded.own)",
            )?
            .execute(params![ns, bucket, own as i64])?;
        Ok(())
    }

    /// Take every dirty `(ns, bucket, own)` and clear the table.
    pub fn take_dirty(&mut self) -> anyhow::Result<Vec<(String, u32, bool)>> {
        let out = {
            let mut stmt = self
                .tx
                .prepare_cached("SELECT ns, bucket, own FROM dirty ORDER BY ns, bucket")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        self.tx.execute("DELETE FROM dirty", [])?;
        Ok(out)
    }

    pub fn projected(&self, k: &LocalKey) -> anyhow::Result<Option<Projected>> {
        let row = self
            .tx
            .prepare_cached(
                "SELECT hash, stamp, scope FROM projected WHERE ns = ?1 AND id = ?2 AND field = ?3",
            )?
            .query_row(params![k.ns, k.id, k.field], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .optional()?;
        let Some((h, s, sc)) = row else {
            return Ok(None);
        };
        Ok(Some((
            hash32(h)?,
            s.as_deref().map(parse_stamp).transpose()?,
            sc.as_deref().map(str::parse::<Scope>).transpose()?,
        )))
    }

    /// `None` removes the row.
    pub fn set_projected(&mut self, k: &LocalKey, v: Option<Projected>) -> anyhow::Result<()> {
        match v {
            None => {
                self.tx
                    .prepare_cached(
                        "DELETE FROM projected WHERE ns = ?1 AND id = ?2 AND field = ?3",
                    )?
                    .execute(params![k.ns, k.id, k.field])?;
            }
            Some((h, s, sc)) => {
                self.tx
                    .prepare_cached(
                        "INSERT OR REPLACE INTO projected (ns, id, field, hash, stamp, scope) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    )?
                    .execute(params![
                        k.ns,
                        k.id,
                        k.field,
                        &h[..],
                        s.map(|s| s.to_string()),
                        sc.map(|s| s.as_str()),
                    ])?;
            }
        }
        Ok(())
    }

    /// Every `projected` row of `ns`.
    pub fn projected_in(&self, ns: &str) -> anyhow::Result<Vec<(LocalKey, Projected)>> {
        let mut stmt = self.tx.prepare_cached(
            "SELECT id, field, hash, stamp, scope FROM projected WHERE ns = ?1 ORDER BY id, field",
        )?;
        let rows = stmt.query_map(params![ns], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, field, h, s, sc) = row?;
            out.push((
                LocalKey::new(ns, id, field),
                (
                    hash32(h)?,
                    s.as_deref().map(parse_stamp).transpose()?,
                    sc.as_deref().map(str::parse::<Scope>).transpose()?,
                ),
            ));
        }
        Ok(out)
    }

    pub fn pin(&self, k: &LocalKey) -> anyhow::Result<Option<(serde_json::Value, PinReason)>> {
        let row = self
            .tx
            .prepare_cached(
                "SELECT value, reason FROM pins WHERE ns = ?1 AND id = ?2 AND field = ?3",
            )?
            .query_row(params![k.ns, k.id, k.field], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
            })
            .optional()?;
        row.map(|(v, reason)| pin_from(v, &reason)).transpose()
    }

    /// `None` removes the pin. Field `*` pins a whole item.
    pub fn set_pin(
        &mut self,
        k: &LocalKey,
        v: Option<(serde_json::Value, PinReason)>,
    ) -> anyhow::Result<()> {
        match v {
            None => {
                self.tx
                    .prepare_cached("DELETE FROM pins WHERE ns = ?1 AND id = ?2 AND field = ?3")?
                    .execute(params![k.ns, k.id, k.field])?;
            }
            Some((value, reason)) => {
                self.tx
                    .prepare_cached(
                        "INSERT OR REPLACE INTO pins (ns, id, field, value, reason) VALUES (?1, ?2, ?3, ?4, ?5)",
                    )?
                    .execute(params![
                        k.ns,
                        k.id,
                        k.field,
                        serde_json::to_vec(&value)?,
                        reason.as_str()
                    ])?;
            }
        }
        Ok(())
    }

    /// Every pin of `ns`, or of every namespace when `ns` is `None`.
    pub fn pins(
        &self,
        ns: Option<&str>,
    ) -> anyhow::Result<Vec<(LocalKey, serde_json::Value, PinReason)>> {
        let mut stmt = self.tx.prepare_cached(
            "SELECT ns, id, field, value, reason FROM pins WHERE ?1 IS NULL OR ns = ?1 ORDER BY ns, id, field",
        )?;
        let rows = stmt.query_map(params![ns], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (ns, id, field, v, reason) = row?;
            let (value, reason) = pin_from(v, &reason)?;
            out.push((LocalKey::new(ns, id, field), value, reason));
        }
        Ok(out)
    }

    /// Whether the shard named `shard` (from any device) is merged already.
    pub fn applied(&self, shard: &str) -> anyhow::Result<bool> {
        Ok(self
            .tx
            .prepare_cached("SELECT 1 FROM applied WHERE name = ?1")?
            .query_row(params![shard], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn add_applied(&mut self, shard: &str) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("INSERT OR IGNORE INTO applied (name) VALUES (?1)")?
            .execute(params![shard])?;
        Ok(())
    }

    /// Forget a merged shard, so it is read again (a rebuild, a rewind).
    pub fn remove_applied(&mut self, shard: &str) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("DELETE FROM applied WHERE name = ?1")?
            .execute(params![shard])?;
        Ok(())
    }

    /// `dev` has seen the tombstone of `ns`/`id` (§8.7).
    pub fn ack(&mut self, ns: &str, id: &str, dev: DeviceId) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("INSERT OR IGNORE INTO acks (ns, id, dev) VALUES (?1, ?2, ?3)")?
            .execute(params![ns, id, dev.to_string()])?;
        Ok(())
    }

    /// The devices that acked the tombstone of `ns`/`id`.
    pub fn acks(&self, ns: &str, id: &str) -> anyhow::Result<Vec<DeviceId>> {
        let mut stmt = self
            .tx
            .prepare_cached("SELECT dev FROM acks WHERE ns = ?1 AND id = ?2 ORDER BY dev")?;
        let rows = stmt.query_map(params![ns, id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for d in rows {
            out.push(d?.parse()?);
        }
        Ok(out)
    }

    /// Drop the acks of a record (its tombstone changed).
    pub fn clear_acks(&mut self, ns: &str, id: &str) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("DELETE FROM acks WHERE ns = ?1 AND id = ?2")?
            .execute(params![ns, id])?;
        Ok(())
    }

    pub fn meta(&self, k: &str) -> anyhow::Result<Option<String>> {
        Ok(self
            .tx
            .prepare_cached("SELECT v FROM meta WHERE k = ?1")?
            .query_row(params![k], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&mut self, k: &str, v: &str) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("INSERT OR REPLACE INTO meta (k, v) VALUES (?1, ?2)")?
            .execute(params![k, v])?;
        Ok(())
    }

    pub fn delete_meta(&mut self, k: &str) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("DELETE FROM meta WHERE k = ?1")?
            .execute(params![k])?;
        Ok(())
    }

    pub fn device(&self, dev: DeviceId) -> anyhow::Result<Option<DeviceRow>> {
        let mut stmt = self.tx.prepare_cached(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE dev = ?1"
        ))?;
        let row = stmt
            .query_row(params![dev.to_string()], device_row)
            .optional()?;
        row.map(device_from).transpose()
    }

    pub fn put_device(&mut self, row: &DeviceRow) -> anyhow::Result<()> {
        self.tx
            .prepare_cached(&format!(
                "INSERT OR REPLACE INTO devices ({DEVICE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ))?
            .execute(params![
                row.dev.to_string(),
                row.name,
                row.os.map(OsKind::as_str),
                row.kage_version,
                to_sql_u64(row.head_seq)?,
                to_sql_u64(row.ingested_seq)?,
                row.last_seen_ms,
                row.instance,
                row.min_reader,
                row.gc_floor.map(|s| s.to_string()),
                row.state,
            ])?;
        Ok(())
    }

    pub fn devices(&self) -> anyhow::Result<Vec<DeviceRow>> {
        let mut stmt = self.tx.prepare_cached(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices ORDER BY dev"
        ))?;
        let rows = stmt.query_map([], device_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(device_from(r?)?);
        }
        Ok(out)
    }

    /// Forget a device (a removed or retired computer).
    pub fn remove_device(&mut self, dev: DeviceId) -> anyhow::Result<()> {
        let d = dev.to_string();
        self.tx
            .prepare_cached("DELETE FROM devices WHERE dev = ?1")?
            .execute(params![d])?;
        self.tx
            .prepare_cached("DELETE FROM acks WHERE dev = ?1")?
            .execute(params![d])?;
        Ok(())
    }

    /// Remove an extension's feed (data sync turned off or purged).
    pub fn feed_clear(&mut self, ext: &str) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("DELETE FROM feed WHERE ext = ?1")?
            .execute(params![ext])?;
        Ok(())
    }

    /// The highest feed sequence number issued for `ext` (0 if none).
    pub fn feed_seq(&self, ext: &str) -> anyhow::Result<u64> {
        Ok(self
            .meta(&feed_seq_key(ext))?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }
}

const DEVICE_COLUMNS: &str =
    "dev, name, os, kage_version, head_seq, ingested_seq, last_seen, instance, min_reader, gc_floor, state";

type RawDevice = (
    String,
    String,
    Option<String>,
    String,
    i64,
    i64,
    i64,
    String,
    u32,
    Option<String>,
    String,
);

fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawDevice> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
    ))
}

fn device_from(r: RawDevice) -> anyhow::Result<DeviceRow> {
    let (
        dev,
        name,
        os,
        kage_version,
        head,
        ingested,
        last_seen,
        instance,
        min_reader,
        floor,
        state,
    ) = r;
    Ok(DeviceRow {
        dev: dev.parse()?,
        name,
        // An OS name a newer build wrote reads as unknown, not as an error.
        os: os.as_deref().and_then(OsKind::from_name),
        kage_version,
        head_seq: head.max(0) as u64,
        ingested_seq: ingested.max(0) as u64,
        last_seen_ms: last_seen,
        instance,
        min_reader,
        gc_floor: floor.as_deref().map(parse_stamp).transpose()?,
        state,
    })
}

fn to_sql_u64(v: u64) -> anyhow::Result<i64> {
    i64::try_from(v).map_err(|_| anyhow::anyhow!("sequence number out of range"))
}

fn pin_from(v: Vec<u8>, reason: &str) -> anyhow::Result<(serde_json::Value, PinReason)> {
    Ok((
        serde_json::from_slice(&v)?,
        PinReason::from_name(reason).ok_or_else(|| anyhow::anyhow!("unknown pin reason"))?,
    ))
}

fn feed_seq_key(ext: &str) -> String {
    format!("feed_seq.{ext}")
}

/// The extension tables (§6.2, §12.4–§12.6), as a trait so the extension
/// source needs no `Tx` lifetimes. `Tx` implements it.
pub trait ExtStore {
    /// Append a change to an extension's feed. Returns its sequence number.
    fn feed_push(&mut self, ext: &str, coll: &str, id: &str) -> anyhow::Result<u64>;
    /// Up to `max` feed rows after `after`: `(seq, collection, id)`.
    fn feed_page(
        &self,
        ext: &str,
        after: u64,
        max: usize,
    ) -> anyhow::Result<Vec<(u64, String, String)>>;
    fn feed_ack(&mut self, ext: &str, upto: u64) -> anyhow::Result<()>;
    /// Per field, the cell the extension last saw: its stamp and value hash.
    fn ext_base(
        &self,
        ext: &str,
        coll: &str,
        id: &str,
    ) -> anyhow::Result<BTreeMap<String, (Stamp, [u8; 32])>>;
    fn set_ext_base(
        &mut self,
        ext: &str,
        coll: &str,
        id: &str,
        f: &BTreeMap<String, (Stamp, [u8; 32])>,
    ) -> anyhow::Result<()>;
    /// Lists mode: the element ids a window last saw for a key.
    fn ext_view(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<BTreeSet<String>>;
    fn set_ext_view(
        &mut self,
        window: &str,
        ext: &str,
        key: &str,
        ids: &BTreeSet<String>,
    ) -> anyhow::Result<()>;
    /// Drop a destroyed window's `reads` and `ext_view` rows.
    fn forget_window(&mut self, window: &str) -> anyhow::Result<()>;
    /// Keys mode: the stamp of the value a window last loaded or saved.
    fn read_stamp(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<Option<Stamp>>;
    fn set_read_stamp(
        &mut self,
        window: &str,
        ext: &str,
        key: &str,
        s: Option<Stamp>,
    ) -> anyhow::Result<()>;
}

/// The feed is coalesced: one row per `(collection, id)`, at the sequence
/// number of its latest change. Sequence numbers only grow, also after
/// every row was acked, so a cursor an extension holds never skips a new
/// change.
impl ExtStore for Tx<'_> {
    fn feed_push(&mut self, ext: &str, coll: &str, id: &str) -> anyhow::Result<u64> {
        let seq = self.feed_seq(ext)? + 1;
        self.set_meta(&feed_seq_key(ext), &seq.to_string())?;
        self.tx
            .prepare_cached("DELETE FROM feed WHERE ext = ?1 AND coll = ?2 AND id = ?3")?
            .execute(params![ext, coll, id])?;
        self.tx
            .prepare_cached("INSERT INTO feed (ext, seq, coll, id) VALUES (?1, ?2, ?3, ?4)")?
            .execute(params![ext, to_sql_u64(seq)?, coll, id])?;
        Ok(seq)
    }

    fn feed_page(
        &self,
        ext: &str,
        after: u64,
        max: usize,
    ) -> anyhow::Result<Vec<(u64, String, String)>> {
        let mut stmt = self.tx.prepare_cached(
            "SELECT seq, coll, id FROM feed WHERE ext = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let after = i64::try_from(after).unwrap_or(i64::MAX);
        let max = i64::try_from(max).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![ext, after, max], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn feed_ack(&mut self, ext: &str, upto: u64) -> anyhow::Result<()> {
        let upto = i64::try_from(upto).unwrap_or(i64::MAX);
        self.tx
            .prepare_cached("DELETE FROM feed WHERE ext = ?1 AND seq <= ?2")?
            .execute(params![ext, upto])?;
        Ok(())
    }

    fn ext_base(
        &self,
        ext: &str,
        coll: &str,
        id: &str,
    ) -> anyhow::Result<BTreeMap<String, (Stamp, [u8; 32])>> {
        let mut stmt = self.tx.prepare_cached(
            "SELECT field, stamp, hash FROM ext_base WHERE ext = ?1 AND coll = ?2 AND id = ?3",
        )?;
        let rows = stmt.query_map(params![ext, coll, id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (f, s, h) = row?;
            out.insert(f, (parse_stamp(&s)?, hash32(h)?));
        }
        Ok(out)
    }

    /// Replaces the record's whole base; an empty map removes it.
    fn set_ext_base(
        &mut self,
        ext: &str,
        coll: &str,
        id: &str,
        f: &BTreeMap<String, (Stamp, [u8; 32])>,
    ) -> anyhow::Result<()> {
        self.tx
            .prepare_cached("DELETE FROM ext_base WHERE ext = ?1 AND coll = ?2 AND id = ?3")?
            .execute(params![ext, coll, id])?;
        let mut ins = self.tx.prepare_cached(
            "INSERT INTO ext_base (ext, coll, id, field, stamp, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (field, (s, h)) in f {
            ins.execute(params![ext, coll, id, field, s.to_string(), &h[..]])?;
        }
        Ok(())
    }

    fn ext_view(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<BTreeSet<String>> {
        let ids: Option<Vec<u8>> = self
            .tx
            .prepare_cached("SELECT ids FROM ext_view WHERE win = ?1 AND ext = ?2 AND key = ?3")?
            .query_row(params![window, ext, key], |r| r.get(0))
            .optional()?;
        Ok(match ids {
            None => BTreeSet::new(),
            Some(b) => serde_json::from_slice(&b)?,
        })
    }

    fn set_ext_view(
        &mut self,
        window: &str,
        ext: &str,
        key: &str,
        ids: &BTreeSet<String>,
    ) -> anyhow::Result<()> {
        self.tx
            .prepare_cached(
                "INSERT OR REPLACE INTO ext_view (win, ext, key, ids) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![window, ext, key, serde_json::to_vec(ids)?])?;
        Ok(())
    }

    fn forget_window(&mut self, window: &str) -> anyhow::Result<()> {
        for sql in [
            "DELETE FROM reads WHERE win = ?1",
            "DELETE FROM ext_view WHERE win = ?1",
        ] {
            self.tx.prepare_cached(sql)?.execute(params![window])?;
        }
        Ok(())
    }

    fn read_stamp(&self, window: &str, ext: &str, key: &str) -> anyhow::Result<Option<Stamp>> {
        let s: Option<String> = self
            .tx
            .prepare_cached("SELECT stamp FROM reads WHERE win = ?1 AND ext = ?2 AND key = ?3")?
            .query_row(params![window, ext, key], |r| r.get(0))
            .optional()?;
        s.as_deref().map(parse_stamp).transpose()
    }

    fn set_read_stamp(
        &mut self,
        window: &str,
        ext: &str,
        key: &str,
        s: Option<Stamp>,
    ) -> anyhow::Result<()> {
        match s {
            None => {
                self.tx
                    .prepare_cached("DELETE FROM reads WHERE win = ?1 AND ext = ?2 AND key = ?3")?
                    .execute(params![window, ext, key])?;
            }
            Some(s) => {
                self.tx
                    .prepare_cached("INSERT OR REPLACE INTO reads (win, ext, key, stamp) VALUES (?1, ?2, ?3, ?4)")?
                    .execute(params![window, ext, key, s.to_string()])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dev(n: u8) -> DeviceId {
        DeviceId([n; 10])
    }

    fn st(hlc: u64, d: u8) -> Stamp {
        Stamp::new(hlc, dev(d))
    }

    fn record(ns: &str, id: &str, n: u64) -> Record {
        let mut r = Record::new(ns, id);
        r.cells.insert(
            FieldKey::all("name"),
            Cell {
                value: json!(format!("item {n}")),
                stamp: st(n, 1),
                wall_ms: n as i64,
                anc: vec![],
            },
        );
        r.cells.insert(
            FieldKey::os("shortcut", OsKind::Macos),
            Cell {
                value: json!({ "keys": ["Cmd", "K"], "f": 1.0 }),
                stamp: st(n + 1, 2),
                wall_ms: 5,
                anc: vec![st(n, 1), st(1, 3)],
            },
        );
        r
    }

    #[test]
    fn open_creates_reopens_and_migrates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sync").join("replica.db");
        let (mut r, note) = Replica::open(&path).unwrap();
        assert_eq!(note, OpenNote::Fresh);
        r.tx(|tx| tx.set_meta("my_seq", "4")).unwrap();
        drop(r);
        let (mut r, note) = Replica::open(&path).unwrap();
        assert_eq!(note, OpenNote::Existing);
        assert_eq!(r.tx(|tx| tx.meta("my_seq")).unwrap().as_deref(), Some("4"));
        let v: u32 = r
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let mode: String = r
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_replica_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replica.db");
        let (mut r, _) = Replica::open(&path).unwrap();
        r.tx(|tx| tx.set_meta("k", "v")).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_and_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replica.db");
        std::fs::write(
            &path,
            b"this is not a database, just some bytes ".repeat(200),
        )
        .unwrap();
        std::fs::write(dir.path().join("replica.db-wal"), b"stale").unwrap();
        let (mut r, note) = Replica::open(&path).unwrap();
        assert_eq!(note, OpenNote::RebuiltFromCorrupt);
        assert!(dir.path().join("replica.db.corrupt-1").exists());
        // The stale WAL went aside with it (or SQLite dropped it); it never
        // belongs to the new database.
        let wal = dir.path().join("replica.db-wal");
        assert!(!wal.exists() || std::fs::read(&wal).unwrap() != b"stale");
        r.tx(|tx| tx.put_record(&record("cfg", "ui.theme", 1), 0))
            .unwrap();
        drop(r);
        let (_, note) = Replica::open(&path).unwrap();
        assert_eq!(note, OpenNote::Existing);

        // A second corrupt file gets the next free name.
        std::fs::write(&path, b"garbage again ".repeat(400)).unwrap();
        let (_, note) = Replica::open(&path).unwrap();
        assert_eq!(note, OpenNote::RebuiltFromCorrupt);
        assert!(dir.path().join("replica.db.corrupt-2").exists());
    }

    #[test]
    fn a_replica_from_a_newer_build_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replica.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
                .unwrap();
        }
        let (_, note) = Replica::open(&path).unwrap();
        assert_eq!(note, OpenNote::RebuiltFromCorrupt);
        assert!(dir.path().join("replica.db.corrupt-1").exists());
    }

    #[test]
    fn some_other_database_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replica.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE meta (x INTEGER);")
                .unwrap();
        }
        let (mut r, note) = Replica::open(&path).unwrap();
        assert_eq!(note, OpenNote::RebuiltFromCorrupt);
        r.tx(|tx| tx.set_meta("k", "v")).unwrap();
    }

    #[test]
    fn only_damaged_files_count_as_unusable() {
        assert!(is_unusable(&Unusable("x").into()));
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        );
        assert!(!is_unusable(&anyhow::Error::from(busy).context("opening")));
        let corrupt = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            None,
        );
        assert!(is_unusable(
            &anyhow::Error::from(corrupt).context("opening")
        ));
    }

    #[test]
    fn records_round_trip_and_select_by_bucket() {
        let mut r = Replica::open_in_memory().unwrap();
        r.tx(|tx| {
            for i in 0..64u32 {
                let mut rec = record("list.shortcuts", &format!("s{i:03}"), u64::from(i) + 10);
                if i % 5 == 0 {
                    rec.del = Some(Tomb {
                        stamp: st(1_000 + u64::from(i), 2),
                        wall_ms: 9,
                        anc: vec![st(10, 1)],
                    });
                }
                tx.put_record(&rec, i * 7919 % 65_536)?;
            }
            tx.put_record(&record("cfg", "ui.theme", 3), 5)
        })
        .unwrap();
        r.tx(|tx| {
            let one = tx.record("list.shortcuts", "s005")?.unwrap();
            let mut want = record("list.shortcuts", "s005", 15);
            want.del = Some(Tomb {
                stamp: st(1_005, 2),
                wall_ms: 9,
                anc: vec![st(10, 1)],
            });
            assert_eq!(one, want);
            // The stored value reads back exactly (1.0 stays a float).
            let v = &one.cells[&FieldKey::os("shortcut", OsKind::Macos)].value;
            assert!(v["f"].is_f64());
            assert!(tx.record("list.shortcuts", "nope")?.is_none());

            let all = tx.records("list.shortcuts")?;
            assert_eq!(all.len(), 64);
            assert!(all.windows(2).all(|w| w[0].id < w[1].id));
            for of in [1u32, 2, 4, 16, 256] {
                let mut seen = 0;
                for b in 0..of {
                    let part = tx.records_in_bucket("list.shortcuts", b, of)?;
                    for rec in &part {
                        let hash = tx.bucket_of(&rec.ns, &rec.id)?.unwrap();
                        assert_eq!(hash % of, b);
                        assert_eq!(rec, all.iter().find(|x| x.id == rec.id).unwrap());
                    }
                    seen += part.len();
                }
                assert_eq!(seen, 64, "every record in exactly one bucket of {of}");
            }
            assert!(tx.records_in_bucket("list.shortcuts", 0, 3).is_err());
            assert_eq!(tx.namespaces()?, vec!["cfg", "list.shortcuts"]);
            assert_eq!(tx.count_records("list.shortcuts")?, 64);
            tx.delete_record("list.shortcuts", "s000")?;
            assert_eq!(tx.count_records("list.shortcuts")?, 63);
            assert!(tx.record("list.shortcuts", "s000")?.is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn put_record_replaces_cells_and_keeps_published() {
        let mut r = Replica::open_in_memory().unwrap();
        r.tx(|tx| {
            tx.put_record(&record("cfg", "a", 1), 1)?;
            tx.mark_published("cfg")?;
            assert!(tx.is_published("cfg", "a")?);
            let mut fewer = Record::new("cfg", "a");
            fewer.cells.insert(
                FieldKey::all("v"),
                Cell {
                    value: json!(null),
                    stamp: st(9, 1),
                    wall_ms: 1,
                    anc: vec![],
                },
            );
            tx.put_record(&fewer, 1)?;
            assert_eq!(tx.record("cfg", "a")?.unwrap(), fewer);
            assert!(tx.is_published("cfg", "a")?);
            assert!(!tx.is_published("cfg", "b")?);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_failed_transaction_changes_nothing() {
        let mut r = Replica::open_in_memory().unwrap();
        let err: anyhow::Result<()> = r.tx(|tx| {
            tx.put_record(&record("cfg", "a", 1), 1)?;
            tx.set_meta("hlc_last", "5")?;
            anyhow::bail!("stop here")
        });
        assert!(err.is_err());
        r.tx(|tx| {
            assert!(tx.record("cfg", "a")?.is_none());
            assert!(tx.meta("hlc_last")?.is_none());
            Ok(())
        })
        .unwrap();
        // A panic inside rolls back too.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = r.tx(|tx| -> anyhow::Result<()> {
                tx.set_meta("k", "v")?;
                panic!("mid_merge_txn");
            });
        }));
        assert!(caught.is_err());
        assert!(r.tx(|tx| tx.meta("k")).unwrap().is_none());
    }

    #[test]
    fn merge_stores_changes_and_marks_echo_dirty() {
        let mut r = Replica::open_in_memory().unwrap();
        r.tx(|tx| {
            let mine = record("cfg", "x", 10);
            tx.write(&mine, 3)?;
            let mut theirs = Record::new("cfg", "x");
            theirs.cells.insert(
                FieldKey::all("name"),
                Cell {
                    value: json!("theirs"),
                    stamp: st(20, 4),
                    wall_ms: 20,
                    anc: vec![],
                },
            );
            let out = tx.merge(&theirs, 3)?;
            assert!(out.changed);
            assert_eq!(out.conflicts.len(), 1);
            let stored = tx.record("cfg", "x")?.unwrap();
            assert_eq!(stored.cells[&FieldKey::all("name")].value, json!("theirs"));
            assert_eq!(tx.take_dirty()?, vec![("cfg".to_string(), 3, true)]);
            // Merging the same again changes nothing.
            let out = tx.merge(&theirs, 3)?;
            assert!(!out.changed);
            assert!(tx.take_dirty()?.is_empty());
            // A new record from elsewhere is an echo.
            let out = tx.merge(&record("cfg", "y", 1), 8)?;
            assert!(out.changed && out.conflicts.is_empty());
            assert_eq!(tx.take_dirty()?, vec![("cfg".to_string(), 8, false)]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn small_tables_round_trip() {
        let mut r = Replica::open_in_memory().unwrap();
        r.tx(|tx| {
            // dirty: once own, stays own.
            tx.mark_dirty("cfg", 1, false)?;
            tx.mark_dirty("cfg", 1, true)?;
            tx.mark_dirty("cfg", 1, false)?;
            tx.mark_dirty("list.macros", 0, false)?;
            assert_eq!(
                tx.take_dirty()?,
                vec![("cfg".into(), 1, true), ("list.macros".into(), 0, false)]
            );
            assert!(tx.take_dirty()?.is_empty());

            // projected
            let k = LocalKey::new("cfg", "ui.theme", "v");
            assert!(tx.projected(&k)?.is_none());
            let p = ([7u8; 32], Some(st(3, 1)), Some(Scope::Os(OsKind::Linux)));
            tx.set_projected(&k, Some(p))?;
            assert_eq!(tx.projected(&k)?, Some(p));
            tx.set_projected(
                &LocalKey::new("cfg", "ui.lang", "v"),
                Some(([1; 32], None, None)),
            )?;
            assert_eq!(tx.projected_in("cfg")?.len(), 2);
            tx.set_projected(&k, None)?;
            assert!(tx.projected(&k)?.is_none());

            // pins
            let star = LocalKey::new("list.shortcuts", "s1", "*");
            tx.set_pin(&star, Some((json!(true), PinReason::ThisComputer)))?;
            tx.set_pin(&k, Some((json!("dark"), PinReason::Lossy)))?;
            assert_eq!(tx.pin(&star)?, Some((json!(true), PinReason::ThisComputer)));
            assert_eq!(tx.pins(None)?.len(), 2);
            assert_eq!(
                tx.pins(Some("cfg"))?,
                vec![(k.clone(), json!("dark"), PinReason::Lossy)]
            );
            tx.set_pin(&star, None)?;
            assert!(tx.pin(&star)?.is_none());

            // applied
            assert!(!tx.applied("3f1c")?);
            tx.add_applied("3f1c")?;
            tx.add_applied("3f1c")?;
            assert!(tx.applied("3f1c")?);
            tx.remove_applied("3f1c")?;
            assert!(!tx.applied("3f1c")?);

            // acks
            tx.ack("cfg", "a", dev(2))?;
            tx.ack("cfg", "a", dev(1))?;
            tx.ack("cfg", "a", dev(1))?;
            assert_eq!(tx.acks("cfg", "a")?, vec![dev(1), dev(2)]);
            tx.clear_acks("cfg", "a")?;
            assert!(tx.acks("cfg", "a")?.is_empty());

            // meta
            tx.set_meta("hlc_last", "1")?;
            tx.set_meta("hlc_last", "2")?;
            assert_eq!(tx.meta("hlc_last")?.as_deref(), Some("2"));
            tx.delete_meta("hlc_last")?;
            assert!(tx.meta("hlc_last")?.is_none());

            // devices
            let row = DeviceRow {
                dev: dev(5),
                name: "Work laptop".into(),
                os: Some(OsKind::Windows),
                kage_version: "0.12.1".into(),
                head_seq: 42,
                ingested_seq: 41,
                last_seen_ms: 1_762_686_000_000,
                instance: "c1d93f0a7b2e4d61".into(),
                min_reader: 1,
                gc_floor: Some(st(77, 9)),
                state: "ok".into(),
            };
            tx.put_device(&row)?;
            tx.put_device(&DeviceRow {
                dev: dev(3),
                ..DeviceRow::default()
            })?;
            assert_eq!(tx.device(dev(5))?, Some(row.clone()));
            assert_eq!(tx.devices()?.len(), 2);
            assert_eq!(tx.devices()?[1], row);
            tx.ack("cfg", "b", dev(5))?;
            tx.remove_device(dev(5))?;
            assert!(tx.device(dev(5))?.is_none());
            assert!(tx.acks("cfg", "b")?.is_empty());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn the_feed_coalesces_and_never_reuses_a_sequence_number() {
        let mut r = Replica::open_in_memory().unwrap();
        r.tx(|tx| {
            assert_eq!(tx.feed_push("mindmap", "items", "a")?, 1);
            assert_eq!(tx.feed_push("mindmap", "items", "b")?, 2);
            assert_eq!(tx.feed_push("mindmap", "items", "a")?, 3);
            assert_eq!(tx.feed_push("todos", "items", "a")?, 1);
            assert_eq!(
                tx.feed_page("mindmap", 0, 10)?,
                vec![
                    (2, "items".into(), "b".into()),
                    (3, "items".into(), "a".into())
                ]
            );
            assert_eq!(tx.feed_page("mindmap", 2, 10)?.len(), 1);
            assert_eq!(tx.feed_page("mindmap", 0, 1)?.len(), 1);
            tx.feed_ack("mindmap", 3)?;
            assert!(tx.feed_page("mindmap", 0, 10)?.is_empty());
            assert_eq!(tx.feed_push("mindmap", "bodies", "a")?, 4);
            assert_eq!(tx.feed_page("mindmap", 3, 10)?.len(), 1);
            assert_eq!(tx.feed_page("todos", 0, 10)?.len(), 1);
            tx.feed_clear("mindmap")?;
            assert!(tx.feed_page("mindmap", 0, 10)?.is_empty());
            assert_eq!(tx.feed_seq("mindmap")?, 4);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn extension_bases_views_and_reads() {
        let mut r = Replica::open_in_memory().unwrap();
        r.tx(|tx| {
            let base: BTreeMap<String, (Stamp, [u8; 32])> = [
                ("+status".to_string(), (st(1, 1), [1; 32])),
                ("text".to_string(), (st(2, 2), [2; 32])),
            ]
            .into_iter()
            .collect();
            tx.set_ext_base("mindmap", "items", "i1", &base)?;
            assert_eq!(tx.ext_base("mindmap", "items", "i1")?, base);
            assert!(tx.ext_base("mindmap", "items", "i2")?.is_empty());
            tx.set_ext_base("mindmap", "items", "i1", &BTreeMap::new())?;
            assert!(tx.ext_base("mindmap", "items", "i1")?.is_empty());

            let ids: BTreeSet<String> = ["a".to_string(), "b".to_string()].into();
            tx.set_ext_view("main", "todos", "todos", &ids)?;
            tx.set_ext_view("floating", "todos", "todos", &BTreeSet::new())?;
            assert_eq!(tx.ext_view("main", "todos", "todos")?, ids);
            assert!(tx.ext_view("settings", "todos", "todos")?.is_empty());

            tx.set_read_stamp("main", "todos", "prefs", Some(st(4, 1)))?;
            assert_eq!(tx.read_stamp("main", "todos", "prefs")?, Some(st(4, 1)));
            tx.set_read_stamp("main", "todos", "prefs", None)?;
            assert!(tx.read_stamp("main", "todos", "prefs")?.is_none());
            tx.set_read_stamp("main", "todos", "prefs", Some(st(5, 1)))?;

            tx.forget_window("main")?;
            assert!(tx.ext_view("main", "todos", "todos")?.is_empty());
            assert!(tx.read_stamp("main", "todos", "prefs")?.is_none());
            Ok(())
        })
        .unwrap();
    }

    /// §20 WP-S1: 10,000 records merged in under 1 second. Timing is only
    /// meaningful in an optimised build.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "benchmark: run with --release")]
    fn bench_ten_thousand_records_merge_under_a_second() {
        let dir = tempfile::tempdir().unwrap();
        let (mut r, _) = Replica::open(&dir.path().join("replica.db")).unwrap();
        let incoming: Vec<Record> = (0..10_000u64)
            .map(|i| record("ext.mindmap.c.items", &format!("i{i:05}"), i + 1))
            .collect();
        let started = std::time::Instant::now();
        r.tx(|tx| {
            for (i, rec) in incoming.iter().enumerate() {
                tx.merge(rec, i as u32 % 65_536)?;
            }
            Ok(())
        })
        .unwrap();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(1), "took {took:?}");
        assert_eq!(
            r.tx(|tx| tx.count_records("ext.mindmap.c.items")).unwrap(),
            10_000
        );
    }
}

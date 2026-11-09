//! Newer and older Kage in one folder (docs/design/SYNC.md §8.11, §18.2
//! I18). A forger writes what a newer Kage would: namespaces, settings,
//! fields and values this build doesn't know, a newer schema and a newer
//! reader version. Each test isolates user dirs first.

mod sync_support;

use kage::sync::canonical::canonical;
use kage::sync::crypto::{unwrap_dek, Keys};
use kage::sync::envelope::{self, Aad, FileKind};
use kage::sync::folder::{self, Layout};
use kage::sync::fsio::{SyncFs, READ_TIMEOUT};
use kage::sync::publish::{HeadFile, NsEntry, ShardFile};
use kage::sync::status::SyncState;
use kage::sync::types::{Cell, DeviceId, FieldKey, OsKind, Record, Scope, Stamp};
use kage::sync::JoinChoice;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use sync_support::*;

/// A computer running a newer Kage, writing straight into the folder.
struct Forger {
    keys: Keys,
    folder_id: String,
    dev: DeviceId,
    seq: u64,
    hlc: u64,
}

impl Forger {
    fn new(c: &SimComputer) -> Self {
        let layout = Layout::in_chosen(&PathBuf::from(CHOSEN));
        let fs: &dyn SyncFs = &*c.fs;
        let d = folder::read_descriptor(fs, &layout).unwrap().unwrap();
        let (_, kf) = folder::list_key_files(fs, &layout).unwrap().remove(0);
        let dek = unwrap_dek(PASS, &kf, &d.folder_id).unwrap();
        Self {
            keys: Keys::derive(&dek),
            folder_id: d.folder_id,
            dev: DeviceId::random(),
            seq: 0,
            hlc: kage::sync::hlc::physical(T0 + 60_000),
        }
    }

    fn cell(&mut self, v: Value) -> Cell {
        self.hlc += 1;
        Cell {
            value: v,
            stamp: Stamp::new(self.hlc, self.dev),
            wall_ms: T0 + 60_000,
            anc: vec![],
        }
    }

    fn dir(&self) -> String {
        format!("{CHOSEN}/Kage Sync/devices/{}", self.dev)
    }

    /// Publish `namespaces` (each a schema and records) as a head with
    /// `min_reader`, into `c`'s view of the folder.
    fn publish(
        &mut self,
        c: &SimComputer,
        namespaces: Vec<(&str, u32, Vec<Record>)>,
        min_reader: u32,
    ) {
        let mut entries = BTreeMap::new();
        for (ns, schema, records) in namespaces {
            let shard = ShardFile {
                ns: ns.to_string(),
                schema,
                bucket: 0,
                of: 1,
                records: records.iter().map(Record::to_json).collect(),
            };
            let plain = canonical(&serde_json::to_value(&shard).unwrap());
            let name = self.keys.content_name(&plain);
            let file = folder::data_name(&name);
            let sealed = envelope::seal(
                &self.keys,
                1,
                &Aad {
                    folder_id: &self.folder_id,
                    device: self.dev,
                    kind: FileKind::Shard,
                    name: &file,
                },
                &plain,
            );
            c.fs.put(format!("{}/shards/{file}", self.dir()), sealed);
            entries.insert(
                ns.to_string(),
                NsEntry {
                    schema,
                    buckets: 1,
                    shards: vec![name],
                },
            );
        }
        self.seq += 1;
        let head = HeadFile {
            format: 1,
            min_reader,
            device: self.dev.to_string(),
            instance: "0011223344556677".into(),
            seq: self.seq,
            written_at: T0 + 60_000,
            max_stamp: Stamp::new(self.hlc, self.dev),
            kage_version: "9.0.0+future".into(),
            catalog: 2,
            os: "linux".into(),
            name: "Future".into(),
            retired: false,
            gc_floor: None,
            seen: BTreeMap::new(),
            namespaces: entries,
        };
        let plain = canonical(&serde_json::to_value(&head).unwrap());
        let file = folder::head_name(self.seq);
        let sealed = envelope::seal(
            &self.keys,
            1,
            &Aad {
                folder_id: &self.folder_id,
                device: self.dev,
                kind: FileKind::Head,
                name: &file,
            },
            &plain,
        );
        c.fs.put(format!("{}/{file}", self.dir()), sealed);
    }

    /// Read computer `c`'s newest head and the records of `ns` it lists.
    fn read(&self, c: &SimComputer, ns: &str) -> (HeadFile, Vec<Record>) {
        let dev = c.device_id();
        let dir = format!("{CHOSEN}/Kage Sync/devices/{dev}");
        let head_path =
            c.fs.files_under(&dir)
                .into_iter()
                .map(|(p, _)| p)
                .filter(|p| {
                    p.parent().unwrap().to_string_lossy() == dir
                        && p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("head-")
                })
                .max()
                .unwrap();
        let open = |path: &PathBuf, kind: FileKind| {
            let bytes = c.fs.read(path, u64::MAX, READ_TIMEOUT).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let keys = self.keys.clone();
            envelope::open(
                &move |_| Some(keys.clone()),
                &Aad {
                    folder_id: &self.folder_id,
                    device: dev.parse().unwrap(),
                    kind,
                    name: &name,
                },
                &bytes,
            )
            .unwrap()
        };
        let head: HeadFile = serde_json::from_slice(&open(&head_path, FileKind::Head)).unwrap();
        let mut out = Vec::new();
        if let Some(e) = head.namespaces.get(ns) {
            for s in &e.shards {
                let p = PathBuf::from(format!("{dir}/shards/{s}.kage"));
                let shard: ShardFile = serde_json::from_slice(&open(&p, FileKind::Shard)).unwrap();
                for r in shard.records {
                    out.push(Record::from_json(ns, &r).unwrap());
                }
            }
        }
        (head, out)
    }
}

fn record(ns: &str, id: &str, cells: Vec<(FieldKey, Cell)>) -> Record {
    let mut r = Record::new(ns, id);
    r.cells = cells.into_iter().collect();
    r
}

/// I18: what an older build doesn't know survives its edits; values it
/// can't hold are never exported back; a newer reader version is waited
/// for; a newer schema is read-only.
#[test]
fn i18_newer_data_survives_an_older_build() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    a.edit(|c| {
        c.shortcuts = vec![serde_json::from_value(json!({
            "name": "gh", "action_type": "open_url", "url": "https://github.com"
        }))
        .unwrap()]
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    let sid = a.config().shortcuts[0].sync_id.clone();
    let mut f = Forger::new(&a);
    let future_ns = record(
        "future.thing",
        "x1",
        vec![(FieldKey::all("v"), f.cell(json!("opaque")))],
    );
    let future_cfg = record(
        "cfg",
        "ui.future_setting",
        vec![(FieldKey::all("v"), f.cell(json!(true)))],
    );
    let channel = record(
        "cfg",
        "updates.channel",
        vec![(FieldKey::all("v"), f.cell(json!("nightly")))],
    );
    let item = record(
        "list.shortcuts",
        &sid,
        vec![(
            FieldKey::all("future_field"),
            f.cell(json!({ "keep": "me" })),
        )],
    );
    f.publish(
        &a,
        vec![
            ("future.thing", 1, vec![future_ns]),
            ("cfg", 1, vec![future_cfg, channel]),
            ("list.shortcuts", 1, vec![item]),
        ],
        1,
    );
    cloud.settle(&mut [&mut a, &mut b], 3);
    // An unknown enum value isn't applied as something else here.
    assert_eq!(a.config().updates.channel, kage::config::Channel::default());
    // The older build edits the same item and the channel row's record.
    a.edit(|c| c.shortcuts[0].name = "GitHub".into());
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().shortcuts[0].name, "GitHub");
    let (head, opaque) = f.read(&a, "future.thing");
    assert!(head.namespaces.contains_key("future.thing"), "republished");
    assert_eq!(opaque.len(), 1);
    let (_, cfg) = f.read(&a, "cfg");
    let get = |id: &str| cfg.iter().find(|r| r.id == id).cloned().unwrap();
    assert_eq!(
        get("ui.future_setting")
            .cell("v", Scope::All)
            .unwrap()
            .value,
        json!(true),
        "an unknown setting survives"
    );
    assert_eq!(
        get("updates.channel").cell("v", Scope::All).unwrap().value,
        json!("nightly"),
        "a value this build can't hold is never exported back"
    );
    let (_, items) = f.read(&a, "list.shortcuts");
    let it = items.iter().find(|r| r.id == sid).unwrap();
    assert_eq!(
        it.cell("future_field", Scope::All).unwrap().value,
        json!({ "keep": "me" }),
        "an unknown field survives an older editor"
    );
    assert_eq!(it.cell("name", Scope::All).unwrap().value, json!("GitHub"));
    assert!(a
        .status()
        .problems
        .iter()
        .any(|p| p.kind == "not_this_version"));
}

/// A head that needs a newer reader isn't applied; a descriptor that does
/// pauses sync with "update Kage".
#[test]
fn newer_reader_versions_wait_for_an_update() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    let mut f = Forger::new(&a);
    let theme = record(
        "cfg",
        "ui.theme",
        vec![(FieldKey::all("v"), f.cell(json!("dark")))],
    );
    f.publish(&a, vec![("cfg", 1, vec![theme])], 2);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(a.config().ui.theme, "system", "not read");
    let dev = a
        .status()
        .devices
        .into_iter()
        .find(|d| d.name == "Future")
        .unwrap();
    assert_eq!(dev.state, kage::sync::status::DeviceState::NeedsUpdate);
    // The folder itself needs a newer reader.
    let desc = format!("{CHOSEN}/Kage Sync/kage-sync.json");
    let mut d: Value = serde_json::from_slice(&a.fs.get(&desc).unwrap()).unwrap();
    d["min_reader"] = json!(2);
    a.fs.put(&desc, serde_json::to_vec(&d).unwrap());
    a.tick();
    assert_eq!(a.status().state, SyncState::UpdateRequired);
}

/// A namespace with a newer schema is read-only here: merged and
/// republished, never projected, local edits not captured.
#[test]
fn a_newer_schema_is_read_only() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    let mut f = Forger::new(&a);
    let item = record(
        "list.shortcuts",
        "sfuture",
        vec![
            (FieldKey::all("name"), f.cell(json!("from the future"))),
            (FieldKey::all("avail"), f.cell(json!("all"))),
        ],
    );
    f.publish(&a, vec![("list.shortcuts", 2, vec![item])], 1);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert!(a.config().shortcuts.is_empty(), "not projected");
    a.edit(|c| {
        c.shortcuts =
            vec![
                serde_json::from_value(json!({ "name": "local", "action_type": "open_url" }))
                    .unwrap(),
            ]
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert!(b.config().shortcuts.is_empty(), "not captured");
    let (_, items) = f.read(&a, "list.shortcuts");
    assert!(items.iter().any(|r| r.id == "sfuture"), "republished");
}

/// Keep the OS import in use.
#[allow(dead_code)]
const _OS: OsKind = OsKind::Linux;

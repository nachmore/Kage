//! Extension sync on a simulated cloud folder (docs/design/SYNC.md §12,
//! §18.2 I25, I26, I31, I32): installs, settings, and data in keys, lists
//! and collections mode, between computers that each run the extension
//! source next to WP-S3's host sources.
//!
//! Every test isolates user dirs first (`sync_support::clock`), so nothing
//! touches real user folders. Extension "windows" are simulated: a save
//! writes the file under the key's lock and tells the journal, as
//! `save_extension_data` does.

#[path = "sync_support/mod.rs"]
mod sync_support;

use kage::extensions::InstalledItem;
use kage::sync::engine::{EngineEnv, SyncEngine, Wake};
use kage::sync::sources::extensions::{store_origin, ExtensionsSource, Journal};
use kage::sync::testing::{FakeClock, FakeCloudFiles, FaultyFs, MemoryKeyStore};
use kage::sync::types::{ConflictKind, Origin, OsKind};
use kage::sync::{
    EngineReply, EngineRequest, ExtReply, ExtRequest, ExtSyncStatus, JoinChoice, PullResult,
    PushResult, RejectCode,
};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use sync_support::*;

const EXT: &str = "notes";

fn store() -> String {
    store_origin("https://store.example/kage")
}

/// A manifest with every data mode and a few kinds of settings.
fn notes(version: &str) -> Value {
    json!({
        "id": EXT, "name": "Notes", "version": version, "type": "extension",
        "permissions": ["storage", "sync"],
        "config": {
            "units": { "type": "string", "enum": ["metric", "imperial"] },
            "city": { "type": "string", "sync": "shared" },
            "organize": { "type": "boolean", "sync": "machine" },
            "api_key": { "type": "string", "sensitive": true }
        },
        "sync": {
            "keys": ["prefs"],
            "lists": { "todos": { "array": "items", "id": "id", "label": "title" } },
            "collections": { "items": { "label": "title", "sets": ["tags"], "local": ["rev"] } }
        }
    })
}

/// An engine with sync off, holding no lock, while a real one is replaced.
fn placeholder() -> SyncEngine {
    let dir = tempfile::tempdir().unwrap();
    let env = EngineEnv {
        local_dir: dir.keep(),
        os: OsKind::Linux,
        kage_version: String::new(),
        fingerprint: String::new(),
        fs: Arc::new(FaultyFs::new()),
        cloud: Arc::new(FakeCloudFiles::new()),
        keys: Arc::new(MemoryKeyStore::new()),
        clock: Arc::new(FakeClock::new(T0)),
        events: Arc::new(Events::default()),
        config: Arc::new(Port::default()),
    };
    SyncEngine::open(env, Vec::new()).unwrap()
}

fn data_dir(c: &SimComputer) -> PathBuf {
    c.config_dir.join("extension-data")
}

/// Restart `c`'s engine with the extension source on `journal`.
fn reopen(c: &mut SimComputer, journal: &Arc<Journal>) {
    let env = EngineEnv {
        local_dir: c.local.clone(),
        os: c.os,
        kage_version: "0.12.0+test".to_string(),
        fingerprint: c.fingerprint.clone(),
        fs: c.fs.clone(),
        cloud: c.cloud.clone(),
        keys: c.keys.clone(),
        clock: c.clock.clone(),
        events: c.events.clone(),
        config: c.port.clone(),
    };
    c.engine = placeholder();
    let mut sources = c.sources();
    sources.push(Box::new(ExtensionsSource::with_journal(
        c.port.clone(),
        data_dir(c),
        None,
        journal.clone(),
    )));
    let mut e = SyncEngine::open(env, sources).unwrap();
    e.set_kdf_cost(TEST_KDF);
    c.engine = e;
}

fn wake(c: &mut SimComputer) {
    c.engine.note_wake(Wake::Local {
        source: "extensions",
        origin: Origin::User,
    });
}

/// Two computers on one folder, both with the extension source.
fn ext_pair() -> (
    SimCloud,
    SimComputer,
    SimComputer,
    Arc<Journal>,
    Arc<Journal>,
) {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
    let (ja, jb): (Arc<Journal>, Arc<Journal>) = (Arc::default(), Arc::default());
    reopen(&mut a, &ja);
    reopen(&mut b, &jb);
    let mut cloud = SimCloud::new();
    a.create();
    cloud.step(&mut [&mut a, &mut b]);
    b.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b], 3);
    (cloud, a, b, ja, jb)
}

/// Install `manifest` with every capability it asks for, from `origin`.
fn install(c: &mut SimComputer, manifest: Value, origin: &str) {
    let item: InstalledItem = InstalledItem {
        manifest: serde_json::from_value(manifest).unwrap(),
        path: format!("/extensions/{EXT}"),
        enabled: true,
    };
    let id = item.manifest.id.clone();
    let perms: Vec<String> = item.manifest.permissions.clone().unwrap_or_default();
    c.port
        .installed
        .lock()
        .unwrap()
        .retain(|i| i.manifest.id != id);
    c.port.installed.lock().unwrap().push(item);
    let origin = origin.to_string();
    c.port.edit(|cfg| {
        let caps: Vec<&str> = perms.iter().map(String::as_str).collect();
        grant(cfg, &id, &caps);
        cfg.extension_grants.get_mut(&id).unwrap().origin = Some(origin);
        cfg.extension_states.insert(id.clone(), true);
    });
    wake(c);
}

/// Uninstall as `uninstall_extension` does: files, settings, grant, data.
fn uninstall(c: &mut SimComputer) {
    c.port
        .installed
        .lock()
        .unwrap()
        .retain(|i| i.manifest.id != EXT);
    c.port.edit(|cfg| {
        cfg.extensions.remove(EXT);
        cfg.extension_states.remove(EXT);
        cfg.extension_grants.remove(EXT);
    });
    let _ = std::fs::remove_dir_all(data_dir(c).join(EXT));
    wake(c);
}

fn key_path(c: &SimComputer, key: &str) -> PathBuf {
    data_dir(c).join(EXT).join(format!("{key}.json"))
}

/// A window's `save_extension_data`.
fn save(c: &mut SimComputer, j: &Journal, window: &str, key: &str, v: &Value) {
    let path = key_path(c, key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = serde_json::to_string(v).unwrap();
    {
        let lock = kage::extensions::data_key_lock(EXT, key);
        let _held = lock.lock().unwrap();
        std::fs::write(&path, &text).unwrap();
        j.note_save(EXT, key, window, Some(&text));
    }
    wake(c);
}

/// A window's `load_extension_data`.
fn load(c: &SimComputer, j: &Journal, window: &str, key: &str) -> Option<Value> {
    let text = std::fs::read_to_string(key_path(c, key)).ok();
    j.note_load(EXT, key, window, text.as_deref());
    text.map(|t| serde_json::from_str(&t).unwrap())
}

fn file(c: &SimComputer, key: &str) -> Option<Value> {
    std::fs::read_to_string(key_path(c, key))
        .ok()
        .map(|t| serde_json::from_str(&t).unwrap())
}

fn ext(c: &mut SimComputer, req: ExtRequest) -> ExtReply {
    match c.ask(EngineRequest::Ext(req)) {
        EngineReply::Ext(r) => r,
        _ => panic!("not an extension reply"),
    }
}

fn status(c: &mut SimComputer) -> ExtSyncStatus {
    match ext(c, ExtRequest::Status { ext: EXT.into() }) {
        ExtReply::Status(s) => s,
        _ => panic!("not a status"),
    }
}

fn push(c: &mut SimComputer, records: Vec<Value>, deleted: Vec<&str>, initial: bool) -> PushResult {
    match ext(
        c,
        ExtRequest::Push {
            ext: EXT.into(),
            collection: "items".into(),
            records,
            deleted: deleted.into_iter().map(str::to_string).collect(),
            initial,
        },
    ) {
        ExtReply::Push(r) => r,
        _ => panic!("not a push result"),
    }
}

fn pull(c: &mut SimComputer) -> PullResult {
    match ext(
        c,
        ExtRequest::Pull {
            ext: EXT.into(),
            max: 500,
        },
    ) {
        ExtReply::Pull(r) => r,
        _ => panic!("not a pull result"),
    }
}

fn ack(c: &mut SimComputer, cursor: &str) {
    ext(
        c,
        ExtRequest::Ack {
            ext: EXT.into(),
            cursor: cursor.into(),
        },
    );
}

/// Pull everything and ack it, as an instance applying the changes does.
fn pull_all(c: &mut SimComputer) -> Vec<(String, Option<Value>)> {
    let mut out = Vec::new();
    loop {
        let p = pull(c);
        out.extend(p.changes.into_iter().map(|ch| (ch.id, ch.record)));
        ack(c, &p.cursor);
        if !p.more {
            return out;
        }
    }
}

fn ext_conflicts(c: &mut SimComputer) -> Vec<kage::sync::status::ConflictSummary> {
    c.conflicts()
        .into_iter()
        .filter(|x| x.category == kage::sync::types::Category::Extensions)
        .collect()
}

/// I25: a collections-mode extension with two instances per computer.
#[test]
fn i25_collections_push_pull_and_ack() {
    let (mut cloud, mut a, mut b, _ja, _jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    install(&mut b, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 2);

    let s = status(&mut a);
    assert!(s.available && s.needs_initial_push, "{s:?}");
    assert_eq!(s.device_tag.len(), 24);
    assert_eq!(s.devices.len(), 2, "both computers, by tag only");

    // The initial push, stamped from the records' own time.
    let r = push(
        &mut a,
        vec![
            json!({ "id": "n1", "title": "Milk", "tags": ["shop"], "rev": 7, "updatedAt": T0 - 60_000 }),
            json!({ "id": "bad id" }),
            json!({ "id": "n9", "password": "x" }),
        ],
        vec![],
        true,
    );
    assert_eq!(r.accepted, 1, "{r:?}");
    assert_eq!(
        r.rejected.iter().map(|x| x.code).collect::<Vec<_>>(),
        [RejectCode::BadId, RejectCode::SensitiveField]
    );
    assert!(
        pull_all(&mut a).is_empty(),
        "A's own push isn't echoed back"
    );
    assert!(!status(&mut a).needs_initial_push);
    cloud.settle(&mut [&mut a, &mut b], 2);

    // Two instances on B pull: delivery is at least once.
    let p1 = pull(&mut b);
    let p2 = pull(&mut b);
    assert_eq!(p1, p2);
    assert_eq!(p1.changes.len(), 1);
    let rec = p1.changes[0].record.clone().unwrap();
    assert_eq!(
        rec,
        json!({ "id": "n1", "title": "Milk", "tags": ["shop"], "updatedAt": T0 - 60_000 }),
        "local fields stay home"
    );
    assert_eq!(p1.changes[0].from.device, status(&mut a).device_tag);
    ack(&mut b, &p1.cursor);
    ack(&mut b, &p2.cursor);
    assert!(pull(&mut b).changes.is_empty(), "acked by either instance");

    // Pushing what is already merged stores nothing (no echo loop).
    let r = push(
        &mut b,
        vec![
            json!({ "id": "n1", "title": "Milk", "tags": ["shop"], "rev": 9, "updatedAt": T0 - 60_000 }),
        ],
        vec![],
        false,
    );
    assert_eq!((r.accepted, r.unchanged), (0, 1));

    // A stale push is a conflict, never a silent overwrite.
    advance(&a.clock, 60_000);
    push(
        &mut a,
        vec![json!({ "id": "n1", "title": "Oat milk", "tags": ["shop"] })],
        vec![],
        false,
    );
    cloud.settle(&mut [&mut a, &mut b], 2);
    advance(&a.clock, 60_000);
    let r = push(
        &mut b,
        vec![json!({ "id": "n1", "title": "Soy milk", "tags": ["shop"] })],
        vec![],
        false,
    );
    assert_eq!(r.accepted, 1);
    let c = ext_conflicts(&mut b);
    assert_eq!(c.len(), 1, "B hadn't pulled A's edit");
    assert_eq!(c[0].kind, ConflictKind::ConcurrentEdit);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(
        pull_all(&mut a),
        [(
            "n1".to_string(),
            Some(json!({ "id": "n1", "title": "Soy milk", "tags": ["shop"] }))
        )]
    );
    // A pushed field equal to its base wasn't changed: the newer value stays.
    let before = pull_all(&mut b);
    assert!(before.is_empty());

    // An initial push from a computer whose copy is older loses to the
    // folder's newer one, and the extension gets the newer one by pull.
    advance(&a.clock, 60_000);
    push(
        &mut a,
        vec![json!({ "id": "n2", "title": "New", "updatedAt": T0 + 100_000 })],
        vec![],
        false,
    );
    cloud.settle(&mut [&mut a, &mut b], 2);
    pull_all(&mut b);
    b.ask(EngineRequest::SetExtensionData {
        ext: EXT.into(),
        on: false,
    });
    b.ask(EngineRequest::SetExtensionData {
        ext: EXT.into(),
        on: true,
    });
    assert!(
        status(&mut b).needs_initial_push,
        "switched on again: push everything once"
    );
    let r = push(
        &mut b,
        vec![json!({ "id": "n2", "title": "Old", "updatedAt": T0 - 3_600_000 })],
        vec![],
        true,
    );
    assert_eq!(r.accepted, 0);
    let got = pull_all(&mut b);
    assert!(
        got.iter()
            .any(|(id, r)| id == "n2" && r.as_ref().unwrap()["title"] == "New"),
        "{got:?}"
    );

    // Deletes.
    let r = push(&mut a, vec![], vec!["n2"], false);
    assert_eq!(r.accepted, 1);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(pull_all(&mut b), [("n2".to_string(), None)]);
}

/// I25: reinstalling brings the data back; purging turns data sync off
/// everywhere without touching local data.
#[test]
fn i25_reinstall_restores_and_purge_keeps_local_data() {
    let (mut cloud, mut a, mut b, ja, _jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    install(&mut b, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 2);
    push(
        &mut a,
        vec![json!({ "id": "n1", "title": "Milk" })],
        vec![],
        true,
    );
    save(&mut a, &ja, "main", "prefs", &json!({ "zoom": 2 }));
    cloud.settle(&mut [&mut a, &mut b], 3);
    pull_all(&mut b);

    uninstall(&mut b);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert!(!status(&mut b).available);
    let r = push(
        &mut b,
        vec![json!({ "id": "n1", "title": "x" })],
        vec![],
        false,
    );
    assert!(r.off, "not installed: nothing is stored");
    install(&mut b, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert!(status(&mut b).needs_initial_push);
    assert_eq!(
        pull_all(&mut b),
        [(
            "n1".to_string(),
            Some(json!({ "id": "n1", "title": "Milk" }))
        )],
        "the first pull after a reinstall returns everything"
    );
    assert_eq!(
        file(&b, "prefs"),
        Some(json!({ "zoom": 2 })),
        "keys come back too"
    );

    a.ask(EngineRequest::PurgeExtension(EXT.into()));
    cloud.settle(&mut [&mut a, &mut b], 3);
    for c in [&a, &b] {
        let prefs = kage::sync::prefs::DevicePrefs::load(&c.local);
        assert!(prefs.ext_data_is_off(EXT), "{}", c.name);
    }
    assert_eq!(
        file(&a, "prefs"),
        Some(json!({ "zoom": 2 })),
        "local data untouched"
    );
    assert_eq!(file(&b, "prefs"), Some(json!({ "zoom": 2 })));
    assert!(push(&mut a, vec![json!({ "id": "n3" })], vec![], false).off);
}

/// Keys mode: a save from a window that read before a remote change is a
/// conflict; one that saved what it read keeps the newer value (§12.4).
#[test]
fn keys_mode_uses_the_windows_base() {
    let (mut cloud, mut a, mut b, ja, jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    install(&mut b, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 2);
    save(&mut a, &ja, "main", "prefs", &json!({ "zoom": 1 }));
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(file(&b, "prefs"), Some(json!({ "zoom": 1 })));

    // A's floating window reads zoom 1; B changes it to 2; it reaches A.
    assert_eq!(
        load(&a, &ja, "floating", "prefs"),
        Some(json!({ "zoom": 1 }))
    );
    a.tick();
    advance(&a.clock, 60_000);
    save(&mut b, &jb, "main", "prefs", &json!({ "zoom": 2 }));
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(file(&a, "prefs"), Some(json!({ "zoom": 2 })));

    // The stale window saves what it read: the newer value stays.
    save(&mut a, &ja, "floating", "prefs", &json!({ "zoom": 1 }));
    a.tick();
    assert_eq!(
        file(&a, "prefs"),
        Some(json!({ "zoom": 2 })),
        "the file gets the newer value back"
    );
    assert!(ext_conflicts(&mut a).is_empty());

    // A's main window reads zoom 2; B changes it to 3; it reaches A; the
    // stale main window saves an edit over zoom 2. Its edit wins, and B's
    // goes to the conflicts list instead of being erased.
    assert_eq!(load(&a, &ja, "main", "prefs"), Some(json!({ "zoom": 2 })));
    a.tick();
    advance(&a.clock, 60_000);
    save(&mut b, &jb, "main", "prefs", &json!({ "zoom": 3 }));
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(file(&a, "prefs"), Some(json!({ "zoom": 3 })));
    advance(&a.clock, 60_000);
    save(
        &mut a,
        &ja,
        "main",
        "prefs",
        &json!({ "zoom": 2, "dark": true }),
    );
    a.tick();
    let c = ext_conflicts(&mut a);
    assert_eq!(c.len(), 1, "main read the value before B's zoom 3");
    assert_eq!(c[0].kind, ConflictKind::ConcurrentEdit);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(file(&b, "prefs"), Some(json!({ "zoom": 2, "dark": true })));

    // A window that read the newest value saves without a conflict.
    assert_eq!(
        load(&a, &ja, "floating", "prefs"),
        Some(json!({ "zoom": 2, "dark": true }))
    );
    a.tick();
    advance(&a.clock, 60_000);
    save(
        &mut a,
        &ja,
        "floating",
        "prefs",
        &json!({ "zoom": 5, "dark": true }),
    );
    a.tick();
    assert_eq!(ext_conflicts(&mut a).len(), 1);
}

/// I26: a lists-mode save from a stale window keeps what it never saw and
/// deletes only what it saw and dropped, with no echo publish.
#[test]
fn i26_lists_mode_keeps_unseen_elements() {
    let (mut cloud, mut a, mut b, ja, jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    install(&mut b, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 2);
    let t1 = json!({ "id": "t1", "title": "Milk" });
    save(
        &mut a,
        &ja,
        "main",
        "todos",
        &json!({ "v": 1, "items": [t1] }),
    );
    cloud.settle(&mut [&mut a, &mut b], 3);

    // W1 and W2 on A load the list; B adds t2; it reaches A; W2 reloads.
    load(&a, &ja, "w1", "todos");
    load(&a, &ja, "w2", "todos");
    a.tick();
    let mut on_b = file(&b, "todos").unwrap();
    load(&b, &jb, "main", "todos");
    on_b["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "t2", "title": "Bread" }));
    advance(&a.clock, 60_000);
    save(&mut b, &jb, "main", "todos", &on_b);
    cloud.settle(&mut [&mut a, &mut b], 2);
    let ids = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(&file(&a, "todos").unwrap()), ["t1", "t2"]);
    load(&a, &ja, "w2", "todos");
    a.tick();

    // W1 saves its stale copy, unchanged: t2 comes back, nothing publishes.
    save(
        &mut a,
        &ja,
        "w1",
        "todos",
        &json!({ "v": 1, "items": [t1] }),
    );
    let rep = a.tick();
    assert!(!rep.published, "no echo publish: {rep:?}");
    assert_eq!(ids(&file(&a, "todos").unwrap()), ["t1", "t2"]);

    // W1 edits t1 over its stale copy: t2 still stays.
    let t1b = json!({ "id": "t1", "title": "Oat milk" });
    save(
        &mut a,
        &ja,
        "w1",
        "todos",
        &json!({ "v": 1, "items": [t1b] }),
    );
    a.tick();
    let now = file(&a, "todos").unwrap();
    assert_eq!(ids(&now), ["t1", "t2"]);
    assert_eq!(now["items"][0]["title"], "Oat milk");

    // W2 saw t2 and drops it: a real delete, everywhere.
    save(
        &mut a,
        &ja,
        "w2",
        "todos",
        &json!({ "v": 1, "items": [t1b] }),
    );
    a.tick();
    assert_eq!(ids(&file(&a, "todos").unwrap()), ["t1"]);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(ids(&file(&b, "todos").unwrap()), ["t1"]);
    assert_eq!(file(&b, "todos").unwrap()["items"][0]["title"], "Oat milk");
    assert_eq!(
        file(&b, "todos").unwrap()["v"],
        1,
        "the rest of the object syncs too"
    );
}

/// I31: uninstalling on A deletes nothing on B, and reinstalling on A
/// brings its settings and data back.
#[test]
fn i31_uninstall_is_not_delete() {
    let (mut cloud, mut a, mut b, ja, _jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    install(&mut b, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 2);
    save(&mut a, &ja, "main", "prefs", &json!({ "zoom": 2 }));
    save(
        &mut a,
        &ja,
        "main",
        "todos",
        &json!({ "items": [{ "id": "t1" }] }),
    );
    push(
        &mut a,
        vec![json!({ "id": "n1", "title": "Milk" })],
        vec![],
        true,
    );
    a.edit(|c| {
        c.extensions.insert(
            EXT.into(),
            json!({ "units": "imperial", "city": "Oslo", "organize": true, "api_key": "sk-live-123456789" }),
        );
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    pull_all(&mut b);
    assert_eq!(
        b.config().extensions[EXT],
        json!({ "units": "imperial", "city": "Oslo" }),
        "machine and sensitive settings stay home"
    );

    uninstall(&mut a);
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(file(&b, "prefs"), Some(json!({ "zoom": 2 })));
    assert!(file(&b, "todos").is_some());
    assert_eq!(b.config().extensions[EXT]["units"], "imperial");
    assert_eq!(b.config().extension_states.get(EXT), Some(&true));
    assert!(
        pull_all(&mut b).is_empty(),
        "no deletes reach the extension"
    );
    let r = reviews(&mut b);
    assert_eq!(
        r.removals.len(),
        1,
        "Removed on Home Mac [Remove here] [Keep]"
    );
    assert_eq!(r.removals[0].id, format!("extension:{EXT}"));
    assert_eq!(r.removals[0].computer.as_deref(), Some("Home Mac"));

    install(&mut a, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(file(&a, "prefs"), Some(json!({ "zoom": 2 })));
    assert_eq!(
        file(&a, "todos"),
        Some(json!({ "items": [{ "id": "t1" }] }))
    );
    let cfg = a.config().extensions[EXT].clone();
    assert_eq!(
        (cfg["units"].clone(), cfg["city"].clone()),
        (json!("imperial"), json!("Oslo"))
    );
    assert!(status(&mut a).needs_initial_push);
    assert_eq!(pull_all(&mut a).len(), 1);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert!(reviews(&mut b).removals.is_empty(), "back on A");
}

/// I32: a stale sandbox's `save_extension_config` doesn't revert a setting
/// changed on another computer.
#[test]
fn i32_a_stale_settings_save_keeps_the_remote_value() {
    let (mut cloud, mut a, mut b, _ja, _jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    install(&mut b, notes("1.0.0"), &store());
    a.edit(|c| {
        c.extensions
            .insert(EXT.into(), json!({ "units": "metric", "city": "Oslo" }));
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    // A's sandbox got its copy (get_extension_config).
    let copy = a.config().extensions[EXT].clone();
    advance(&a.clock, 60_000);
    b.edit(|c| {
        c.extensions.get_mut(EXT).unwrap()["units"] = json!("imperial");
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(a.config().extensions[EXT]["units"], "imperial");
    // The stale sandbox saves its copy with its own edit.
    let incoming = json!({ "units": "metric", "city": "Bergen" });
    let merged = kage::commands::extensions::merge_extension_settings(
        a.config().extensions.get(EXT),
        Some(&copy),
        incoming,
    );
    advance(&a.clock, 60_000);
    a.edit(|c| {
        c.extensions.insert(EXT.into(), merged);
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    for c in [&a, &b] {
        assert_eq!(
            c.config().extensions[EXT],
            json!({ "units": "imperial", "city": "Bergen" }),
            "{}",
            c.name
        );
    }
}

/// §12.3 condition 5: a different extension that reuses the id gets
/// nothing, and sends nothing.
#[test]
fn a_different_origin_gets_nothing() {
    let (mut cloud, mut a, mut b, ja, _jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 2);
    save(&mut a, &ja, "main", "prefs", &json!({ "zoom": 2 }));
    push(
        &mut a,
        vec![json!({ "id": "n1", "title": "Secret plan" })],
        vec![],
        true,
    );
    a.edit(|c| {
        c.extensions
            .insert(EXT.into(), json!({ "units": "imperial" }));
    });
    cloud.settle(&mut [&mut a, &mut b], 3);

    // B installs another extension called "notes", from a file.
    install(&mut b, notes("9.0.0"), "sideload");
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(file(&b, "prefs"), None);
    assert!(!b.config().extensions.contains_key(EXT));
    assert!(!status(&mut b).available);
    assert!(push(&mut b, vec![json!({ "id": "x" })], vec![], false).off);
    // Even with its data switch on: the other copy came from a store.
    b.ask(EngineRequest::SetExtensionData {
        ext: EXT.into(),
        on: true,
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(file(&b, "prefs"), None);
    assert!(!status(&mut b).available);

    // And another store's "notes" is a different extension too.
    let clock = a.clock.clone();
    let mut c = SimComputer::new("Laptop", OsKind::Linux, clock);
    let jc: Arc<Journal> = Arc::default();
    reopen(&mut c, &jc);
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    c.join(JoinChoice::Theirs);
    install(
        &mut c,
        notes("1.0.0"),
        &store_origin("https://other.example/store"),
    );
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    assert_eq!(file(&c, "prefs"), None);
    assert!(!c.config().extensions.contains_key(EXT));
    // A keeps syncing with itself.
    assert!(status(&mut a).available);
}

/// A sideloaded extension's data syncs only once the user switched it on,
/// on every computer.
#[test]
fn sideloaded_data_waits_for_the_switch() {
    let (mut cloud, mut a, mut b, ja, _jb) = ext_pair();
    install(&mut a, notes("1.0.0"), "sideload");
    install(&mut b, notes("1.0.0"), "sideload");
    cloud.settle(&mut [&mut a, &mut b], 2);
    save(&mut a, &ja, "main", "prefs", &json!({ "zoom": 2 }));
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert!(!status(&mut a).available, "off by default for a sideload");
    assert_eq!(file(&b, "prefs"), None);
    for c in [&mut a, &mut b] {
        c.ask(EngineRequest::SetExtensionData {
            ext: EXT.into(),
            on: true,
        });
    }
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(file(&b, "prefs"), Some(json!({ "zoom": 2 })));
}

/// Installs: a store item on A is a suggestion on B; dismissing it keeps
/// it away.
#[test]
fn installs_become_suggestions_elsewhere() {
    let (mut cloud, mut a, mut b, _ja, _jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 3);
    let r = reviews(&mut b);
    assert_eq!(r.suggestions.len(), 1);
    assert_eq!(r.suggestions[0].id, format!("extension:{EXT}"));
    assert_eq!(r.suggestions[0].label_key, "sync.review.suggestion");
    assert_eq!(b.status().suggestions, 1);
    b.ask(EngineRequest::DismissSuggestion {
        kind: "suggestion".into(),
        id: format!("extension:{EXT}"),
    });
    assert!(reviews(&mut b).suggestions.is_empty());
    assert!(
        b.port.installed.lock().unwrap().is_empty(),
        "sync installs no code"
    );
}

/// Journal order: a save is judged by what its window had seen when it
/// saved, even when the window loads again before sync looks.
#[test]
fn a_save_is_judged_by_what_its_window_had_seen_then() {
    let (mut cloud, mut a, mut b, ja, _jb) = ext_pair();
    install(&mut a, notes("1.0.0"), &store());
    install(&mut b, notes("1.0.0"), &store());
    cloud.settle(&mut [&mut a, &mut b], 2);
    let both = json!({ "items": [{ "id": "t1" }, { "id": "t2" }] });
    save(&mut a, &ja, "main", "todos", &both);
    cloud.settle(&mut [&mut a, &mut b], 3);
    load(&a, &ja, "w", "todos");
    let one = json!({ "items": [{ "id": "t1" }] });
    save(&mut a, &ja, "w", "todos", &one);
    load(&a, &ja, "w", "todos");
    a.tick();
    assert_eq!(
        file(&a, "todos"),
        Some(one.clone()),
        "t2 was seen and dropped"
    );
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(file(&b, "todos"), Some(one));
}

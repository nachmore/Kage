//! Membership and recovery on a simulated cloud folder (docs/design/SYNC.md
//! §18.2: I17, I20, I21, I27, I29, I30; undo joining; a lost replica).
//! Each test isolates user dirs first.

mod sync_support;

use kage::sync::sources::chats::{ChatHistory, ChatMeta};
use kage::sync::status::SyncState;
use kage::sync::types::OsKind;
use kage::sync::{EngineReply, EngineRequest, JoinChoice};
use serde_json::json;
use std::sync::{Arc, Mutex};
use sync_support::*;

/// I20: B's config.json is reset at load. B projects the synced values,
/// publishes none of its defaults, and says so.
#[test]
fn i20_a_reset_config_gets_the_synced_values() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    a.edit(|c| {
        c.ui.theme = "dark".into();
        c.ui.font_size = 17;
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().ui.theme, "dark");
    // B restarts with a config.json that couldn't be read.
    b.port.replace(kage::config::Config::default());
    *b.port.reset.lock().unwrap() = kage::sync::engine::LoadReset::All;
    b.reopen();
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().ui.theme, "dark");
    assert_eq!(b.config().ui.font_size, 17);
    assert_eq!(a.config().ui.theme, "dark", "no defaults spread");
    assert_eq!(a.config().ui.font_size, 17);
    assert!(b.status().problems.iter().any(|p| p.kind == "config_reset"));
    assert!(a.conflicts().is_empty());
}

/// I21: B is a copy of A's profile. B takes a new device id; both edit
/// before they see each other; nothing is lost.
#[test]
fn i21_a_cloned_profile_takes_a_new_id() {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut cloud = SimCloud::new();
    a.create();
    a.edit(|c| c.shortcuts = vec![shortcut("one", "https://1")]);
    cloud.settle(&mut [&mut a], 2);
    let mut b = a.clone_profile("Copy");
    let ids = |c: &SimComputer| {
        c.status()
            .devices
            .iter()
            .find(|d| d.this)
            .map(|d| d.id.clone())
            .unwrap()
    };
    b.tick();
    assert_ne!(ids(&a), ids(&b), "the copy took a new id");
    a.edit(|c| c.ui.theme = "dark".into());
    b.edit(|c| c.shortcuts.push(shortcut("two", "https://2")));
    cloud.settle(&mut [&mut a, &mut b], 4);
    assert_eq!(b.config().ui.theme, "dark");
    let names: Vec<String> = a
        .config()
        .shortcuts
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names, vec!["one", "two"]);
    assert_eq!(a.status().devices.len(), 2);
}

fn shortcut(name: &str, url: &str) -> kage::config::ShortcutConfig {
    serde_json::from_value(json!({ "name": name, "action_type": "open_url", "url": url })).unwrap()
}

/// I27: a passphrase change on A doesn't touch B. A "lost or stolen"
/// removal rotates the key: B is locked until the new passphrase, then
/// syncs on under the new epoch.
#[test]
fn i27_passphrase_change_and_key_rotation() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    let mut c = SimComputer::new("Old laptop", OsKind::Linux, a.clock.clone());
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    c.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    a.ask(EngineRequest::ChangePassphrase(
        "a brand new passphrase".into(),
    ));
    cloud.settle(&mut [&mut a, &mut b, &mut c], 2);
    a.edit(|cfg| cfg.ui.theme = "dark".into());
    cloud.settle(&mut [&mut a, &mut b, &mut c], 2);
    assert_eq!(b.config().ui.theme, "dark", "B didn't notice the change");
    // A fourth computer joins with the new passphrase.
    let mut d = SimComputer::new("New PC", OsKind::Windows, a.clock.clone());
    cloud.step(&mut [&mut a, &mut b, &mut c, &mut d]);
    let r = d.try_ask(EngineRequest::Join(kage::sync::JoinReq {
        path: CHOSEN.into(),
        passphrase: PASS.into(),
        device_name: "New PC".into(),
        categories: Default::default(),
    }));
    assert!(r.is_err(), "the old passphrase is gone");
    d.ask(EngineRequest::Join(kage::sync::JoinReq {
        path: CHOSEN.into(),
        passphrase: "a brand new passphrase".into(),
        device_name: "New PC".into(),
        categories: Default::default(),
    }));
    d.ask(EngineRequest::ConfirmJoin(JoinChoice::Theirs));
    cloud.settle(&mut [&mut a, &mut b, &mut d], 2);
    assert_eq!(d.config().ui.theme, "dark");

    // C is lost: remove it and rotate the key.
    let c_id = c
        .status()
        .devices
        .iter()
        .find(|x| x.this)
        .unwrap()
        .id
        .clone();
    a.ask(EngineRequest::RemoveDevice {
        id: c_id,
        lost: true,
        new_passphrase: Some("rotated passphrase here".into()),
    });
    cloud.settle(&mut [&mut a, &mut b, &mut d], 2);
    assert_eq!(b.status().state, SyncState::Locked);
    a.edit(|cfg| cfg.ui.font_size = 19);
    cloud.settle(&mut [&mut a, &mut b, &mut d], 2);
    assert_ne!(
        b.config().ui.font_size,
        19,
        "B can't read the new epoch yet"
    );
    b.ask(EngineRequest::Unlock {
        passphrase: "rotated passphrase here".into(),
    });
    cloud.settle(&mut [&mut a, &mut b, &mut d], 3);
    assert_eq!(b.config().ui.font_size, 19);
    assert_ne!(b.status().state, SyncState::Locked);
    // The lost computer can't read anything written since: it only knows
    // the key changed.
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    c.tick();
    c.tick();
    assert_eq!(c.status().state, SyncState::Locked, "{:?}", c.status());
    assert_ne!(c.config().ui.font_size, 19);
}

/// Chats of this computer for tests: idle, recent.
struct History(Mutex<Vec<ChatMeta>>);

impl ChatHistory for History {
    fn list(&self) -> Vec<ChatMeta> {
        self.0.lock().unwrap().clone()
    }
    fn load(&self, chat: &ChatMeta) -> anyhow::Result<Vec<serde_json::Value>> {
        Ok(vec![
            json!({ "role": "user", "content": format!("about {}", chat.title) }),
            json!({ "role": "assistant", "content": "sure" }),
        ])
    }
}

/// I29: with chat history on, A's chats appear read-only on B; hiding one
/// on B hides it on A; A's agent files are untouched.
#[test]
fn i29_chats_are_copied_read_only() {
    let recent = chrono::Utc::now().timestamp_millis() - DAY;
    let history = Arc::new(History(Mutex::new(vec![ChatMeta {
        provider: "kiro-cli".into(),
        session_id: "sess-1".into(),
        title: "Release plan".into(),
        created_ms: recent,
        updated_ms: recent,
        locator: json!({}),
    }])));
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    a.chats = Some(history.clone());
    a.reopen();
    let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
    // B has no chats of its own.
    b.chats = Some(Arc::new(History(Mutex::new(Vec::new()))));
    b.reopen();
    let mut cloud = SimCloud::new();
    a.create();
    a.ask(EngineRequest::SetCategory {
        category: kage::sync::types::Category::Chats,
        on: true,
    });
    cloud.step(&mut [&mut a, &mut b]);
    b.join(JoinChoice::Theirs);
    b.ask(EngineRequest::SetCategory {
        category: kage::sync::types::Category::Chats,
        on: true,
    });
    cloud.settle(&mut [&mut a, &mut b], 4);
    let mirrors = kage::sync::sources::chats::list_mirrors(&b.local.join("chats"));
    assert_eq!(mirrors.len(), 1, "B mirrors A's chat");
    assert_eq!(mirrors[0].1.title, "Release plan");
    let transcript = std::fs::read_to_string(&mirrors[0].0).unwrap();
    assert!(transcript.contains("about Release plan"));
    // Hide it on B: delete the mirror.
    std::fs::remove_file(&mirrors[0].0).unwrap();
    std::fs::remove_file(mirrors[0].0.with_extension("json")).unwrap();
    b.engine.note_wake(kage::sync::engine::Wake::Local {
        source: "chats",
        origin: kage::sync::types::Origin::User,
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    let hidden = std::fs::read_to_string(a.local.join("chats/hidden.json")).unwrap_or_default();
    assert!(hidden.contains("sess-1"), "hidden on A too: {hidden}");
    // The agent's list is untouched.
    assert_eq!(history.list().len(), 1);
    assert!(kage::sync::sources::chats::list_mirrors(&b.local.join("chats")).is_empty());
}

/// I30: leave on B, remove C from A, start over on A.
#[test]
fn i30_leave_remove_and_start_over() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    let mut c = SimComputer::new("Old laptop", OsKind::Linux, clock.clone());
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    c.join(JoinChoice::Theirs);
    a.edit(|cfg| cfg.ui.theme = "dark".into());
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    let id_of = |x: &SimComputer| {
        x.status()
            .devices
            .iter()
            .find(|d| d.this)
            .unwrap()
            .id
            .clone()
    };
    let b_id = id_of(&b);
    // B leaves: its local data stays; A cleans up its directory.
    b.ask(EngineRequest::Leave);
    assert_eq!(b.status().state, SyncState::Off);
    assert_eq!(b.config().ui.theme, "dark", "local data stays");
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    a.tick();
    advance(&clock, DAY + 1);
    a.tick();
    cloud.step(&mut [&mut a, &mut c]);
    assert!(
        cloud
            .files(&format!("{CHOSEN}/Kage Sync/devices/{b_id}"))
            .is_empty(),
        "B's directory was removed"
    );
    // Remove C from A: C stops and says so.
    let c_id = id_of(&c);
    a.ask(EngineRequest::RemoveDevice {
        id: c_id,
        lost: false,
        new_passphrase: None,
    });
    cloud.settle(&mut [&mut a, &mut c], 2);
    assert_eq!(c.status().state, SyncState::Removed);
    assert_eq!(c.config().ui.theme, "dark");
    // Start over on A: the others see a reset.
    let mut d = SimComputer::new("Desk", OsKind::Windows, clock.clone());
    cloud.step(&mut [&mut a, &mut d]);
    d.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut d], 2);
    a.ask(EngineRequest::StartOver {
        passphrase: "start again passphrase".into(),
    });
    cloud.settle(&mut [&mut a, &mut d], 2);
    // D wrote one head into the old layout before it saw the reset; A
    // waits for it, then counts it unreadable.
    advance(&clock, 11 * 60_000);
    cloud.settle(&mut [&mut a, &mut d], 1);
    assert_eq!(d.status().state, SyncState::Reset, "{:?}", d.status());
    assert_eq!(a.status().state, SyncState::UpToDate, "{:?}", a.status());
    assert!(!cloud
        .files(&format!("{CHOSEN}/Kage Sync/archive"))
        .is_empty());
    assert_eq!(d.config().ui.theme, "dark", "no local data lost");
}

/// Undo joining puts this computer's own values back and turns sync off
/// here.
#[test]
fn undo_join_restores_this_computers_values() {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
    a.edit(|c| c.ui.theme = "dark".into());
    b.edit(|c| {
        c.ui.theme = "light".into();
        c.shortcuts = vec![shortcut("mine", "https://m")];
    });
    let mut cloud = SimCloud::new();
    a.create();
    cloud.step(&mut [&mut a, &mut b]);
    b.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().ui.theme, "dark");
    assert!(b.status().undo_join_until.is_some());
    b.ask(EngineRequest::UndoJoin);
    assert_eq!(b.config().ui.theme, "light");
    assert_eq!(b.config().shortcuts.len(), 1);
    assert_eq!(b.status().state, SyncState::Off);
}

/// A lost replica is rebuilt from the folder with the other computers'
/// values; local differences become conflicts.
#[test]
fn a_lost_replica_is_rebuilt_from_the_folder() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    a.edit(|c| c.ui.theme = "dark".into());
    cloud.settle(&mut [&mut a, &mut b], 2);
    // B's replica is gone (a disk cleaner), and B changed a value meanwhile.
    b.engine = kage::sync::engine::SyncEngine::open(
        kage::sync::engine::EngineEnv {
            local_dir: b.local.join("../placeholder"),
            os: OsKind::Linux,
            kage_version: String::new(),
            fingerprint: String::new(),
            fs: b.fs.clone(),
            cloud: b.cloud.clone(),
            keys: b.keys.clone(),
            clock: b.clock.clone(),
            events: b.events.clone(),
            config: b.port.clone(),
        },
        Vec::new(),
    )
    .unwrap();
    for f in ["replica.db", "replica.db-wal", "replica.db-shm"] {
        let _ = std::fs::remove_file(b.local.join(f));
    }
    std::fs::write(b.local.join("replica.db"), b"not a database at all, really").unwrap();
    b.port.edit(|c| c.ui.theme = "light".into());
    b.reopen();
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(
        b.config().ui.theme,
        "dark",
        "the other computers' values win"
    );
    assert_eq!(a.config().ui.theme, "dark");
    let listed = b.conflicts();
    assert!(
        listed.iter().any(|c| c.loser.preview == "light"),
        "{listed:?}"
    );
}

/// I17: B was away 120 days. A deleted items and cleaned up their
/// tombstones meanwhile. B's copies go to the conflicts list as
/// "deleted while away" instead of coming back; B's new items publish.
#[test]
fn i17_coming_back_after_months() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    a.edit(|c| c.shortcuts = vec![shortcut("old", "https://o"), shortcut("keep", "https://k")]);
    cloud.settle(&mut [&mut a, &mut b], 3);
    // B goes away; A deletes "old" and lives on for 120 days.
    a.edit(|c| c.shortcuts.retain(|s| s.name != "old"));
    for _ in 0..13 {
        a.tick();
        cloud.step(&mut [&mut a]);
        advance(&clock, 10 * DAY);
    }
    a.tick();
    cloud.step(&mut [&mut a]);
    // B comes back with a new item of its own.
    b.edit(|c| c.shortcuts.push(shortcut("new on B", "https://n")));
    b.reopen();
    cloud.settle(&mut [&mut a, &mut b], 4);
    let names = |c: &SimComputer| -> Vec<String> {
        c.config()
            .shortcuts
            .iter()
            .map(|s| s.name.clone())
            .collect()
    };
    assert!(!names(&a).contains(&"old".to_string()), "{:?}", names(&a));
    assert!(!names(&b).contains(&"old".to_string()), "{:?}", names(&b));
    assert!(names(&a).contains(&"new on B".to_string()));
    let away = match b.ask(EngineRequest::ListConflicts(
        kage::sync::ConflictTab::Replaced,
    )) {
        EngineReply::Conflicts(c) => c,
        _ => unreachable!(),
    };
    assert!(
        away.iter()
            .any(|c| c.kind == kage::sync::types::ConflictKind::DeletedWhileAway),
        "{away:?}"
    );
}

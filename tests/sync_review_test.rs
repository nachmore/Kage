//! Defects found reviewing folder sync against docs/design/SYNC.md: data
//! that didn't reach the other computers, a join choice that was ignored,
//! a key that reached the folder, evicted files read as a rewind, and a
//! removed computer that came back. Each test isolates user dirs first
//! (`sync_support::clock`).

mod sync_support;

use kage::sync::engine::LoadReset;
use kage::sync::status::{DeviceState, SyncState};
use kage::sync::types::OsKind;
use kage::sync::{EngineRequest, JoinChoice};
use serde_json::json;
use std::path::PathBuf;
use sync_support::*;

fn automation(name: &str, enabled: bool) -> kage::config::MacroConfig {
    serde_json::from_value(json!({ "name": name, "steps": [], "enabled": enabled })).unwrap()
}

/// A setting or list field that is `false` here is published like any
/// other value (§8.8 Create step 2: every local syncable value becomes a
/// cell). Before, a `false` with no cell yet was skipped as if it were a
/// removed set element, so a computer that joined kept its own `true`, and
/// a switched-off automation arrived switched on.
#[test]
fn false_values_reach_the_other_computers() {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
    a.edit(|c| {
        c.quick_actions.enabled = false;
        c.ui.mascot_animations = false;
        c.macros = vec![automation("off here", false)];
    });
    let mut cloud = SimCloud::new();
    a.create();
    cloud.step(&mut [&mut a, &mut b]);
    b.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b], 3);
    let cb = b.config();
    assert!(!cb.quick_actions.enabled, "a shared false setting arrives");
    assert!(!cb.ui.mascot_animations);
    assert_eq!(cb.macros.len(), 1);
    assert!(!cb.macros[0].enabled, "a switched-off automation stays off");
}

/// A `config.json` missing at launch (a new install) counts as a reset
/// only while sync was on (§8.10). Setting sync up later in that session
/// must neither show the reset notice nor skip capture: joining with
/// "Use this computer's settings" keeps this computer's values.
#[test]
fn setting_up_after_a_fresh_install_keeps_the_choice() {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
    *a.port.reset.lock().unwrap() = LoadReset::All;
    *b.port.reset.lock().unwrap() = LoadReset::All;
    a.edit(|c| c.ui.theme = "dark".into());
    b.edit(|c| c.ui.theme = "light".into());
    let mut cloud = SimCloud::new();
    a.create();
    assert!(
        !a.status().problems.iter().any(|p| p.kind == "config_reset"),
        "{:?}",
        a.status().problems
    );
    cloud.step(&mut [&mut a, &mut b]);
    b.join(JoinChoice::Mine);
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().ui.theme, "light", "B chose its own settings");
    assert_eq!(a.config().ui.theme, "light");
    assert!(!b.status().problems.iter().any(|p| p.kind == "config_reset"));
}

/// A value that looks like it holds a key stays on its computer (§4.3
/// "Held back", §11.1). Joining with "Use the settings from my other
/// computers" must not put it in the conflicts list, which syncs: it
/// stays here, held back, and the other computers never see it.
#[test]
fn a_held_back_value_never_reaches_the_folder_at_join() {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
    let secret = "https://store.example/catalog.json?token=abc123def456ghi789";
    a.edit(|c| c.store_url = Some("https://store.example/catalog.json".into()));
    b.edit(|c| c.store_url = Some(secret.into()));
    let mut cloud = SimCloud::new();
    a.create();
    cloud.step(&mut [&mut a, &mut b]);
    b.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b], 3);
    for c in a.conflicts().iter().chain(b.conflicts().iter()) {
        assert!(
            !c.loser.preview.contains("token"),
            "a held-back value is in the conflicts list: {c:?}"
        );
    }
    assert_eq!(
        b.config().store_url.as_deref(),
        Some(secret),
        "it stays on B"
    );
    assert_eq!(
        a.config().store_url.as_deref(),
        Some("https://store.example/catalog.json")
    );
}

/// Older macOS evicts iCloud files to `.<name>.icloud` stubs. Kage never
/// needs its own files back (§8.12), so its own evicted heads and shards
/// are not a rewound directory: nothing is published again, and the
/// status doesn't say the sync app keeps undoing Kage's files.
#[test]
fn evicted_own_files_are_not_a_rewind() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    a.edit(|c| c.ui.theme = "dark".into());
    cloud.settle(&mut [&mut a, &mut b], 2);
    let own = a.own_dir();
    let evict = |c: &SimComputer| {
        for (p, _) in c.fs.files_under(&own) {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('~') {
                continue;
            }
            c.fs.delete(&p);
            c.fs.put(
                p.with_file_name(format!(".{name}.icloud")),
                b"stub".to_vec(),
            );
        }
    };
    let heads = |c: &SimComputer| -> Vec<PathBuf> {
        c.fs.files_under(&own)
            .into_iter()
            .map(|(p, _)| p)
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("head-")
            })
            .collect()
    };
    // Take in what the last step brought, so A has nothing new to say.
    a.tick();
    assert!(!a.tick().published, "A is quiet before the eviction");
    assert!(!heads(&a).is_empty());
    evict(&a);
    assert!(heads(&a).is_empty());
    for _ in 0..4 {
        advance(&clock, 60_000);
        let rep = a.tick();
        assert!(!rep.published, "nothing changed, so nothing is published");
    }
    assert!(heads(&a).is_empty(), "no head was written again");
    assert_ne!(a.status().state, SyncState::FolderReverting);
    // Edits still go out.
    a.edit(|c| c.ui.font_size = 18);
    a.tick();
    cloud.step(&mut [&mut a, &mut b]);
    b.tick();
    assert_eq!(b.config().ui.font_size, 18);
}

/// A computer removed from sync stays removed (§8.9), on the computer
/// that removed it and on every other one: a head it publishes before it
/// hears of the removal doesn't bring it back, and no one waits for it
/// as an active computer.
#[test]
fn a_removed_computer_stays_removed_everywhere() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    let mut c = SimComputer::new("Old laptop", OsKind::Linux, clock.clone());
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    c.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    let c_id = c.device_id();
    a.ask(EngineRequest::RemoveDevice {
        id: c_id.clone(),
        lost: false,
        new_passphrase: None,
    });
    // C publishes once more before the removal reaches it.
    c.edit(|cfg| cfg.ui.font_size = 21);
    c.tick();
    advance(&clock, 60_000);
    cloud.settle(&mut [&mut a, &mut b], 3);
    let state_on = |x: &SimComputer| {
        x.status()
            .devices
            .iter()
            .find(|d| d.id == c_id)
            .map(|d| d.state)
    };
    assert_eq!(state_on(&a), Some(DeviceState::Retired), "on A");
    assert_eq!(state_on(&b), Some(DeviceState::Retired), "on B");
}

/// "Lost or stolen" makes a new key epoch (§11.3). Readers accept the old
/// epoch only until every active computer has moved; after that a file
/// sealed with the old key, by the lost computer or by anyone who has its
/// key, changes nothing.
#[test]
fn the_old_key_stops_working_once_every_computer_moved() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    let mut c = SimComputer::new("Lost laptop", OsKind::Linux, clock.clone());
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    c.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    let c_id = c.device_id();
    a.ask(EngineRequest::RemoveDevice {
        id: c_id,
        lost: true,
        new_passphrase: Some("rotated passphrase here".into()),
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    b.ask(EngineRequest::Unlock {
        passphrase: "rotated passphrase here".into(),
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    a.edit(|cfg| cfg.ui.font_size = 19);
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(
        b.config().ui.font_size,
        19,
        "A and B sync under the new key"
    );
    // The lost computer still has the old key, and writes with it.
    advance(&clock, 60_000);
    c.edit(|cfg| cfg.ui.theme = "dark".into());
    c.tick();
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    a.tick();
    b.tick();
    assert_ne!(a.config().ui.theme, "dark", "on A");
    assert_ne!(b.config().ui.theme, "dark", "on B");
    // A and B still sync with each other.
    b.edit(|cfg| cfg.ui.font_size = 20);
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(a.config().ui.font_size, 20);
}

/// When the keychain refuses the key (denied, locked, gone), entering the
/// passphrase still unlocks sync: the key goes to the owner-only file
/// instead (§15 "Keychain entry gone or denied": the file key store is
/// offered), and stays found after a restart.
#[test]
fn a_refused_keychain_falls_back_to_the_file() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    b.keys.fail(Some(kage::sync::testing::KeyFault::Denied));
    b.reopen();
    b.tick();
    assert_eq!(b.status().state, SyncState::Locked);
    b.ask(EngineRequest::Unlock {
        passphrase: PASS.into(),
    });
    assert_ne!(b.status().state, SyncState::Locked, "{:?}", b.status());
    assert_eq!(
        b.status().key_store,
        Some(kage::sync::keystore::KeyStoreKind::File)
    );
    a.edit(|c| c.ui.theme = "dark".into());
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().ui.theme, "dark");
    // After a restart the key is found in the file.
    b.reopen();
    a.edit(|c| c.ui.font_size = 18);
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_ne!(b.status().state, SyncState::Locked);
    assert_eq!(b.config().ui.font_size, 18);
}

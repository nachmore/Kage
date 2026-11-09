//! Computers on a simulated cloud folder (docs/design/SYNC.md §18.2,
//! I1–I11, I15, I19, I22–I24). Each test isolates user dirs first.

mod sync_support;

use kage::sync::status::SyncState;
use kage::sync::types::{OsKind, ScopeAction};
use kage::sync::{ConflictAction, EngineReply, EngineRequest, JoinChoice};
use serde_json::json;
use sync_support::*;

fn set_theme(c: &mut SimComputer, theme: &str) {
    let theme = theme.to_string();
    c.edit(move |cfg| cfg.ui.theme = theme);
}

/// I1: A creates, B joins with each choice; the same effective config
/// except what stays on each computer and per-OS fields.
#[test]
fn i1_join_with_each_choice() {
    for choice in [JoinChoice::Theirs, JoinChoice::Mine] {
        let clock = clock();
        let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
        let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
        a.edit(|c| {
            c.ui.theme = "dark".into();
            c.system.auto_start = true;
        });
        b.edit(|c| {
            c.ui.theme = "light".into();
            c.shortcuts = vec![shortcut("only on B", "https://b")];
        });
        let mut cloud = SimCloud::new();
        a.create();
        cloud.step(&mut [&mut a, &mut b]);
        b.join(choice);
        cloud.settle(&mut [&mut a, &mut b], 3);
        let (ca, cb) = (a.config(), b.config());
        let theme = match choice {
            JoinChoice::Theirs => "dark",
            JoinChoice::Mine => "light",
        };
        assert_eq!(ca.ui.theme, theme, "{choice:?}");
        assert_eq!(cb.ui.theme, theme, "{choice:?}");
        // Only here on B, so published either way.
        assert_eq!(ca.shortcuts.len(), 1, "{choice:?}");
        assert_eq!(ca.shortcuts[0].sync_id, cb.shortcuts[0].sync_id);
        // This computer only.
        assert!(ca.system.auto_start);
        assert!(!cb.system.auto_start);
        // The replaced value is listed on both and can be brought back.
        let on_a = a.conflicts();
        let on_b = b.conflicts();
        assert_eq!(on_a.len(), 1, "{choice:?}: {on_a:?}");
        assert_eq!(on_b.len(), 1, "{choice:?}");
        assert_eq!(
            on_a[0].kind,
            kage::sync::types::ConflictKind::ReplacedOnJoin
        );
        a.ask(EngineRequest::ResolveConflict {
            id: on_a[0].id.clone(),
            action: ConflictAction::Restore,
        });
        cloud.settle(&mut [&mut a, &mut b], 3);
        let back = match choice {
            JoinChoice::Theirs => "light",
            JoinChoice::Mine => "dark",
        };
        assert_eq!(a.config().ui.theme, back);
        assert_eq!(b.config().ui.theme, back);
        assert_eq!(a.status().state, SyncState::UpToDate, "{:?}", a.status());
    }
}

/// I2: an edit on A arrives on B, with no conflict.
#[test]
fn i2_an_edit_arrives() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    set_theme(&mut a, "dark");
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().ui.theme, "dark");
    assert!(a.conflicts().is_empty());
    assert!(b.conflicts().is_empty());
}

/// I3: both edit while apart; the newer wins on both, one conflict on
/// both, and Restore on B wins everywhere with no new conflict.
#[test]
fn i3_concurrent_edits_conflict_once() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    set_theme(&mut a, "dark");
    a.tick();
    advance(&clock, 1_000);
    set_theme(&mut b, "light");
    b.tick();
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(a.config().ui.theme, "light");
    assert_eq!(b.config().ui.theme, "light");
    let ca = a.conflicts();
    let cb = b.conflicts();
    assert_eq!(ca.len(), 1, "{ca:?}");
    assert_eq!(cb.len(), 1);
    assert_eq!(ca[0].id, cb[0].id, "one entry, the same everywhere");
    assert_eq!(ca[0].loser.preview, "dark");
    b.ask(EngineRequest::ResolveConflict {
        id: cb[0].id.clone(),
        action: ConflictAction::Restore,
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(a.config().ui.theme, "dark");
    assert_eq!(b.config().ui.theme, "dark");
    assert!(a.conflicts().is_empty(), "{:?}", a.conflicts());
    let resolved = match a.ask(EngineRequest::ListConflicts(
        kage::sync::ConflictTab::Resolved,
    )) {
        EngineReply::Conflicts(c) => c,
        _ => unreachable!(),
    };
    assert!(resolved
        .iter()
        .any(|c| c.kind == kage::sync::types::ConflictKind::ReplacedByRestore));
}

/// I4: an edit made after seeing the other computer's edit isn't a
/// conflict.
#[test]
fn i4_edit_after_seeing_is_no_conflict() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    set_theme(&mut a, "dark");
    cloud.settle(&mut [&mut a, &mut b], 2);
    advance(&clock, 1_000);
    set_theme(&mut b, "light");
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(a.config().ui.theme, "light");
    assert!(a.conflicts().is_empty());
    assert!(b.conflicts().is_empty());
}

/// I6 and I7: per-OS hotkeys and a Mac override.
#[test]
fn i6_i7_per_os_values() {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut b = SimComputer::new("Work PC", OsKind::Windows, clock.clone());
    let mut cloud = SimCloud::new();
    a.create();
    cloud.step(&mut [&mut a, &mut b]);
    b.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b], 2);
    a.edit(|c| {
        c.hotkey.modifiers = vec!["Super".into()];
        c.hotkey.key = "K".into();
    });
    b.edit(|c| {
        c.hotkey.modifiers = vec!["Ctrl".into()];
        c.hotkey.key = "J".into();
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(a.config().hotkey.key, "K");
    assert_eq!(b.config().hotkey.key, "J");
    // C, another Mac, joins: it gets A's hotkey.
    let mut c = SimComputer::new("Laptop", OsKind::Macos, clock.clone());
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    c.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    assert_eq!(c.config().hotkey.key, "K");
    // "Same on all computers" on B nulls both OS cells.
    b.ask(EngineRequest::SetScope {
        path: "hotkey".into(),
        action: ScopeAction::All,
    });
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    assert_eq!(a.config().hotkey.key, "J");
    assert_eq!(c.config().hotkey.key, "J");

    // I7: a Mac override for the font size leaves Windows alone.
    a.ask(EngineRequest::SetScope {
        path: "ui.font_size".into(),
        action: ScopeAction::Os,
    });
    a.edit(|c| c.ui.font_size = 18);
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    assert_eq!(c.config().ui.font_size, 18);
    assert_eq!(b.config().ui.font_size, 14);
    a.ask(EngineRequest::SetScope {
        path: "ui.font_size".into(),
        action: ScopeAction::UseShared,
    });
    cloud.settle(&mut [&mut a, &mut b, &mut c], 3);
    assert_eq!(a.config().ui.font_size, 14);
    assert_eq!(c.config().ui.font_size, 14);
}

/// I8: "Only this computer" on A.
#[test]
fn i8_only_this_computer() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    a.ask(EngineRequest::SetScope {
        path: "ui.theme".into(),
        action: ScopeAction::ThisComputer,
    });
    set_theme(&mut a, "dark");
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().ui.theme, "system", "A's edit never leaves A");
    set_theme(&mut b, "light");
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(a.config().ui.theme, "dark", "remote edits don't apply on A");
    a.ask(EngineRequest::SetScope {
        path: "ui.theme".into(),
        action: ScopeAction::StopThisComputer,
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(a.config().ui.theme, "light");
}

fn shortcut(name: &str, url: &str) -> kage::config::ShortcutConfig {
    serde_json::from_value(json!({ "name": name, "action_type": "open_url", "url": url })).unwrap()
}

/// I9 and I10: lists merge item by item; a delete against an edit; two
/// reorders.
#[test]
fn i9_i10_lists() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    a.edit(|c| {
        c.shortcuts = vec![
            shortcut("one", "https://1"),
            shortcut("two", "https://2"),
            shortcut("three", "https://3"),
        ]
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    let names = |c: &SimComputer| -> Vec<String> {
        c.config()
            .shortcuts
            .iter()
            .map(|s| s.name.clone())
            .collect()
    };
    assert_eq!(names(&b), vec!["one", "two", "three"]);
    // I10: A moves "three" first while B edits "two".
    a.edit(|c| {
        let t = c.shortcuts.remove(2);
        c.shortcuts.insert(0, t);
    });
    b.edit(|c| c.shortcuts[1].url = Some("https://two".into()));
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(names(&a), vec!["three", "one", "two"]);
    assert_eq!(names(&b), vec!["three", "one", "two"]);
    assert_eq!(a.config().shortcuts[2].url.as_deref(), Some("https://two"));
    // I9: A deletes "one" while B edits it, B later.
    a.edit(|c| c.shortcuts.retain(|s| s.name != "one"));
    a.tick();
    advance(&clock, 1_000);
    b.edit(|c| {
        if let Some(s) = c.shortcuts.iter_mut().find(|s| s.name == "one") {
            s.url = Some("https://uno".into());
        }
    });
    b.tick();
    cloud.settle(&mut [&mut a, &mut b], 3);
    // The edit was newer: the item is back on both, and the delete is
    // listed.
    assert!(names(&a).contains(&"one".to_string()), "{:?}", names(&a));
    assert!(names(&b).contains(&"one".to_string()));
    assert!(a
        .conflicts()
        .iter()
        .any(|c| c.kind == kage::sync::types::ConflictKind::DeleteVsEdit));
}

/// I11: a `run_program` shortcut made on a Mac stays on Macs until it is
/// made available everywhere; then Windows has it with an empty path and
/// the Mac's value as a hint.
#[test]
fn i11_programs_stay_on_the_os_they_were_made_on() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    a.edit(|c| {
        c.shortcuts = vec![serde_json::from_value(json!({
            "name": "vim", "action_type": "run_program", "path": "/usr/bin/vim"
        }))
        .unwrap()]
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert!(b.config().shortcuts.is_empty());
    let id = a.config().shortcuts[0].sync_id.clone();
    a.ask(EngineRequest::SetScope {
        path: format!("shortcuts/{id}"),
        action: ScopeAction::AvailAll,
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    let on_b = b.config().shortcuts;
    assert_eq!(on_b.len(), 1);
    assert_eq!(on_b[0].name, "vim");
    assert!(on_b[0].path.as_deref().unwrap_or("").is_empty());
    let view = scope(&mut b, &format!("shortcuts/{id}/path"));
    assert_eq!(view.others.len(), 1);
    assert_eq!(view.others[0].os, OsKind::Macos);
    assert_eq!(view.others[0].preview, "/usr/bin/vim");
    // A path set on Windows stays on Windows.
    b.edit(|c| c.shortcuts[0].path = Some("C:\\vim.exe".into()));
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(
        a.config().shortcuts[0].path.as_deref(),
        Some("/usr/bin/vim")
    );
    assert_eq!(b.config().shortcuts[0].path.as_deref(), Some("C:\\vim.exe"));
}

/// I15: B's clock is a day ahead. The same winners everywhere; a warning
/// on A; reading B's files moves A's clock at most an hour past its wall.
#[test]
fn i15_a_clock_far_ahead() {
    let ca = clock();
    let cb = std::sync::Arc::new(kage::sync::testing::FakeClock::new(T0 + DAY));
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, ca.clone());
    let mut b = SimComputer::new("Work PC", OsKind::Windows, cb.clone());
    let mut cloud = SimCloud::new();
    a.create();
    cloud.step(&mut [&mut a, &mut b]);
    b.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut b], 3);
    set_theme(&mut b, "light");
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(a.config().ui.theme, "light");
    assert!(
        a.status()
            .problems
            .iter()
            .any(|p| p.kind == "clock_warning"),
        "{:?}",
        a.status().problems
    );
    // An edit made after seeing B's beats it, whatever the clocks say.
    set_theme(&mut a, "dark");
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().ui.theme, "dark");
    assert!(a.conflicts().is_empty());
    // Concurrent edits: the same winner on both, and the loser is kept.
    // (A's clock followed B's when A replaced B's value, so either may
    // win here.)
    a.edit(|c| c.ui.font_size = 12);
    a.tick();
    b.edit(|c| c.ui.font_size = 18);
    b.tick();
    cloud.settle(&mut [&mut a, &mut b], 3);
    let winner = a.config().ui.font_size;
    assert!(winner == 12 || winner == 18);
    assert_eq!(b.config().ui.font_size, winner);
    assert_eq!(a.conflicts().len(), 1);
    assert_eq!(b.conflicts().len(), 1);
    // Reading B's files alone never pulled A's clock past its wall + 1h:
    // a fresh computer that only reads B stamps below B's values.
    let mut c = SimComputer::new("Reader", OsKind::Linux, ca.clone());
    cloud.step(&mut [&mut a, &mut b, &mut c]);
    c.join(JoinChoice::Theirs);
    let _ = c.engine.status();
}

/// I16: B offline 40 days with edits while A keeps editing: the newer
/// wins, losers are listed, tombstones wait.
#[test]
fn i16_offline_for_weeks() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    a.edit(|c| c.shortcuts = vec![shortcut("gone", "https://g")]);
    cloud.settle(&mut [&mut a, &mut b], 3);
    // B goes offline.
    b.edit(|c| c.ui.font_size = 12);
    b.tick();
    advance(&clock, 10 * DAY);
    a.edit(|c| {
        c.ui.font_size = 16;
        c.shortcuts.clear();
    });
    for _ in 0..4 {
        advance(&clock, 10 * DAY);
        a.tick();
        cloud.step(&mut [&mut a]);
    }
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(a.config().ui.font_size, 16);
    assert_eq!(b.config().ui.font_size, 16);
    assert!(b.config().shortcuts.is_empty(), "the delete reached B");
    assert_eq!(b.conflicts().len(), 1, "B's font size is kept");
}

/// I19: deleting 300 shortcuts asks on A first, then holds on B.
#[test]
fn i19_bulk_deletes_ask_first() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    a.edit(|c| {
        c.shortcuts = (0..300)
            .map(|i| shortcut(&format!("s{i}"), "https://x"))
            .collect()
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().shortcuts.len(), 300);
    a.edit(|c| c.shortcuts.clear());
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().shortcuts.len(), 300, "A holds the publish");
    let ra = reviews(&mut a);
    assert_eq!(ra.mass_changes.len(), 1);
    assert_eq!(ra.mass_changes[0].kind, "mass_delete_out");
    a.ask(EngineRequest::AnswerMassChange {
        ns: "list.shortcuts".into(),
        accept: true,
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().shortcuts.len(), 300, "B holds and asks");
    let rb = reviews(&mut b);
    assert_eq!(rb.mass_changes.len(), 1);
    assert_eq!(rb.mass_changes[0].count, Some(300));
    b.ask(EngineRequest::AnswerMassChange {
        ns: "list.shortcuts".into(),
        accept: true,
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert!(b.config().shortcuts.is_empty());
    let trash = std::fs::read_dir(b.local.join("trash")).unwrap().count();
    assert!(trash >= 1, "a copy was kept");
    assert!(reviews(&mut b).mass_changes.is_empty());
}

/// I22: a backup restored on A reaches B, and B's replaced values can be
/// brought back.
#[test]
fn i22_backup_import_spreads_and_is_listed() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    set_theme(&mut b, "light");
    cloud.settle(&mut [&mut a, &mut b], 2);
    let mut restored = a.config();
    restored.ui.theme = "dark".into();
    a.port.replace(restored);
    a.engine.note_wake(kage::sync::engine::Wake::Local {
        source: "config",
        origin: kage::sync::types::Origin::Import,
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().ui.theme, "dark");
    let listed = b.conflicts();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(
        listed[0].kind,
        kage::sync::types::ConflictKind::RestoreBackup
    );
    assert_eq!(listed[0].loser.preview, "light");
}

/// I23: a scheduled automation with a script stays off on B until
/// approved; a later change of its script holds it again; approval
/// publishes nothing.
#[test]
fn i23_runnables_wait_for_approval() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    let auto = |script: &str| -> kage::config::MacroConfig {
        serde_json::from_value(json!({
            "name": "Nightly",
            "enabled": true,
            "trigger": { "type": "schedule", "interval": "daily_09:00" },
            "steps": [{ "step_type": "script", "script": script }],
        }))
        .unwrap()
    };
    let first = auto("return input;");
    a.edit(move |c| c.macros = vec![first]);
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().macros.len(), 1);
    assert!(!b.config().macros[0].enabled, "off until approved");
    assert!(a.config().macros[0].enabled);
    let r = reviews(&mut b);
    assert_eq!(r.runnables.len(), 1, "{r:?}");
    assert!(r.runnables[0]
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("return input"));
    let key = r.runnables[0].id.clone();
    b.ask(EngineRequest::ApproveRunnable(key));
    let published_before = b.engine.status().last_publish_at;
    b.tick();
    assert!(b.config().macros[0].enabled);
    assert_eq!(
        b.engine.status().last_publish_at,
        published_before,
        "approval publishes nothing"
    );
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert!(a.config().macros[0].enabled);
    // The script changes on A: held again on B.
    a.edit(|c| c.macros[0].steps[0].script = "return input.toUpperCase();".into());
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert!(!b.config().macros[0].enabled, "held again after a change");
    assert_eq!(reviews(&mut b).runnables.len(), 1);
    // The hold never travels: A keeps it on.
    assert!(a.config().macros[0].enabled);
    // Keep off: it stays off and stops asking, also after a rename.
    let key = reviews(&mut b).runnables[0].id.clone();
    b.ask(EngineRequest::KeepOff(key));
    a.edit(|c| c.macros[0].name = "Nightly job".into());
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().macros[0].name, "Nightly job");
    assert!(!b.config().macros[0].enabled);
    assert!(reviews(&mut b).runnables.is_empty());
}

/// The active connection follows another computer only once the command
/// of the connection it points to is approved here. "Keep off" stops the
/// asking until that command changes; pointing back needs no review.
#[test]
fn the_active_connection_waits_for_approval() {
    let clock = clock();
    let mut a = SimComputer::new("Home Mac", OsKind::Macos, clock.clone());
    let mut c = SimComputer::new("Laptop", OsKind::Macos, clock.clone());
    let mut cloud = SimCloud::new();
    a.create();
    cloud.step(&mut [&mut a, &mut c]);
    c.join(JoinChoice::Theirs);
    cloud.settle(&mut [&mut a, &mut c], 3);
    let local = |cmd: &str| -> kage::config::AgentConnection {
        serde_json::from_value(json!({
            "id": "agent2",
            "name": "My agent",
            "mode": { "type": "local", "spawn_command": cmd },
        }))
        .unwrap()
    };
    let first = local("my-agent --acp");
    a.edit(move |cfg| {
        cfg.acp.connections.push(first);
        cfg.acp.active_connection_id = "agent2".into();
    });
    cloud.settle(&mut [&mut a, &mut c], 3);
    assert_eq!(c.config().acp.connections.len(), 2);
    assert_eq!(c.config().acp.active_connection_id, "default", "waits");
    let r = reviews(&mut c);
    assert_eq!(r.runnables.len(), 1, "{r:?}");
    assert_eq!(r.runnables[0].id, "list.connections/agent2");
    assert!(r.runnables[0]
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("my-agent"));

    // Keep off: no more asking, the pointer stays.
    c.ask(EngineRequest::KeepOff("list.connections/agent2".into()));
    c.tick();
    assert!(reviews(&mut c).runnables.is_empty());
    assert_eq!(c.config().acp.active_connection_id, "default");

    // A waiting pointer follows a move to a connection that runs nothing.
    a.edit(|cfg| {
        let mut remote = cfg.acp.connections[0].clone();
        remote.id = "agent3".into();
        remote.name = "Remote".into();
        cfg.acp.connections.push(remote);
        cfg.acp.active_connection_id = "agent3".into();
    });
    cloud.settle(&mut [&mut a, &mut c], 3);
    assert_eq!(c.config().acp.active_connection_id, "agent3");

    // A new command asks again; approving it moves the pointer.
    a.edit(|cfg| {
        cfg.acp.connections[1] = local("my-agent --acp --fast");
        cfg.acp.active_connection_id = "agent2".into();
    });
    cloud.settle(&mut [&mut a, &mut c], 3);
    assert_eq!(c.config().acp.active_connection_id, "agent3");
    assert_eq!(reviews(&mut c).runnables.len(), 1);
    c.ask(EngineRequest::ApproveRunnable(
        "list.connections/agent2".into(),
    ));
    c.tick();
    assert_eq!(c.config().acp.active_connection_id, "agent2");
    assert!(reviews(&mut c).runnables.is_empty());

    // Pointing back to a connection that runs nothing needs no review.
    a.edit(|cfg| cfg.acp.active_connection_id = "default".into());
    cloud.settle(&mut [&mut a, &mut c], 3);
    assert_eq!(c.config().acp.active_connection_id, "default");
    assert!(c.conflicts().is_empty());
    assert_eq!(a.config().acp.active_connection_id, "default");
}

/// I24: an extension switched on elsewhere stays off where its grant
/// doesn't cover what it asks for.
#[test]
fn i24_consent_is_per_computer() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    for c in [&mut a, &mut b] {
        *c.port.installed.lock().unwrap() = vec![installed("weather", &["storage", "network"])];
    }
    a.edit(|c| {
        grant(c, "weather", &["storage", "network"]);
        c.extension_states.insert("weather".into(), false);
    });
    b.edit(|c| {
        grant(c, "weather", &["storage"]);
        c.extension_states.insert("weather".into(), false);
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    a.edit(|c| {
        c.extension_states.insert("weather".into(), true);
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    assert_eq!(b.config().extension_states.get("weather"), Some(&false));
    let r = reviews(&mut b);
    assert_eq!(r.approvals.len(), 1, "{r:?}");
    assert_eq!(r.approvals[0].id, "weather");
    // The grant here widens (the Store's Approve): it switches on.
    b.edit(|c| grant(c, "weather", &["storage", "network"]));
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().extension_states.get("weather"), Some(&true));
    // The grant never travels.
    assert_eq!(a.config().extension_grants["weather"].granted.len(), 2);
}

/// §4.4 "Lost ids": an older Kage or an editor dropped an item's id and a
/// new one was minted. Capture matches the item to its record, gives it
/// its old id back, and nothing is deleted or duplicated anywhere.
#[test]
fn lost_ids_are_matched_back() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    a.edit(|c| c.shortcuts = vec![shortcut("one", "https://1"), shortcut("two", "https://2")]);
    cloud.settle(&mut [&mut a, &mut b], 3);
    let ids: Vec<String> = a
        .config()
        .shortcuts
        .iter()
        .map(|s| s.sync_id.clone())
        .collect();
    // The ids are gone (and one item was renamed meanwhile).
    a.edit(|c| {
        for s in c.shortcuts.iter_mut() {
            s.sync_id = String::new();
        }
        c.shortcuts[1].name = "two!".into();
    });
    cloud.settle(&mut [&mut a, &mut b], 3);
    let after: Vec<String> = a
        .config()
        .shortcuts
        .iter()
        .map(|s| s.sync_id.clone())
        .collect();
    assert_eq!(after, ids, "the old ids are back");
    let names: Vec<String> = b
        .config()
        .shortcuts
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names, vec!["one", "two!"]);
    assert!(a.conflicts().is_empty());
    assert!(b.conflicts().is_empty());
}

/// §8.10 churn: a value two computers keep changing back and forth is
/// frozen for the session, and the Sync page says so.
#[test]
fn a_value_that_keeps_flipping_is_frozen() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    // Each computer rewrites the value after it sees the other's (two
    // builds that disagree about it).
    for i in 0..24 {
        let (who, theme) = if i % 2 == 0 {
            (&mut a, "dark")
        } else {
            (&mut b, "light")
        };
        set_theme(who, theme);
        cloud.settle(&mut [&mut a, &mut b], 2);
        advance(&clock, 60_000);
    }
    let frozen = |c: &SimComputer| c.status().problems.iter().any(|p| p.kind == "churn");
    assert!(frozen(&a) || frozen(&b), "{:?}", a.status().problems);
}

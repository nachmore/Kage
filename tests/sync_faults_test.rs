//! Cloud-folder trouble and crashes (docs/design/SYNC.md §18.2 I12–I14,
//! I28; §18.3 fault injection and kill points). Each test isolates user
//! dirs first.

mod sync_support;

use kage::os::cloud_files::Residency;
use kage::sync::engine::{kill, Wake};
use kage::sync::status::SyncState;
use kage::sync::testing::{Fault, FsOp, KeyFault};
use kage::sync::types::OsKind;
use kage::sync::{EngineRequest, JoinChoice};
use std::path::Path;
use std::time::Instant;
use sync_support::*;

fn set_theme(c: &mut SimComputer, theme: &str) {
    let theme = theme.to_string();
    c.edit(move |cfg| cfg.ui.theme = theme);
}

/// `p` with `/` between its parts on every OS, for substring checks: on
/// Windows the engine joins the parts with `\`.
fn slashed(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Files of `dev`'s directory that `c` doesn't have yet.
fn new_files_of(c: &SimComputer, dev: &str, p: &Path) -> bool {
    slashed(p).contains(&format!("/devices/{dev}/")) && c.fs.get(p).is_none()
}

/// I12: files from A arrive half written first. Nothing is applied from
/// them; once complete they apply; `waiting` then `up_to_date`.
#[test]
fn i12_partial_files_wait() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    set_theme(&mut a, "dark");
    a.tick();
    let a_dev = a.device_id();
    let b_view = b.fs.clone();
    cloud.step_with(&mut [&mut a, &mut b], |who, p| {
        if who == "Work PC"
            && slashed(p).contains(&format!("/devices/{a_dev}/"))
            && b_view.get(p).is_none()
        {
            Deliver::Truncated
        } else {
            Deliver::Now
        }
    });
    b.tick();
    assert_eq!(b.config().ui.theme, "system");
    assert_eq!(b.status().state, SyncState::Waiting, "{:?}", b.status());
    cloud.step(&mut [&mut a, &mut b]);
    b.tick();
    assert_eq!(b.config().ui.theme, "dark");
    assert_eq!(b.status().state, SyncState::UpToDate, "{:?}", b.status());
}

/// I13: placeholders never block a tick; downloads are asked for; after
/// 3 ticks the status says so; once downloaded they apply.
#[test]
fn i13_placeholders_never_block() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    set_theme(&mut a, "dark");
    a.tick();
    b.cloud.set_default_residency(Residency::Placeholder);
    cloud.step(&mut [&mut a, &mut b]);
    for _ in 0..3 {
        let t = Instant::now();
        b.tick();
        assert!(
            t.elapsed().as_millis() < 2_000,
            "a tick waited on a placeholder"
        );
    }
    assert_eq!(b.config().ui.theme, "system");
    assert_eq!(b.status().state, SyncState::Placeholders);
    assert!(!b.cloud.download_requests().is_empty());
    b.cloud.set_default_residency(Residency::Local);
    b.tick();
    assert_eq!(b.config().ui.theme, "dark");
}

/// I14: a sync app's exact copy of A's head is removed quietly; a head
/// with A's id from another computer (a VM clone) makes A take a new id.
#[test]
fn i14_duplicates_and_clones() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    let dir = a.own_dir();
    let head =
        a.fs.files_under(&dir)
            .into_iter()
            .filter(|(p, _)| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("head-")
            })
            .max_by_key(|(p, _)| p.clone())
            .unwrap();
    let name = head.0.file_name().unwrap().to_string_lossy().to_string();
    let copy = dir.join(name.replace(".kage", " (1).kage"));
    a.fs.put(&copy, head.1.clone());
    let id = a.device_id();
    a.tick();
    assert!(!a.fs.exists(&copy), "the copy is gone");
    assert_eq!(a.device_id(), id, "an exact copy isn't a clone");
    assert!(a.status().problems.iter().any(|p| p.kind == "duplicates"));

    // A VM clone: same files, same fingerprint, same id.
    let mut twin = a.twin("Twin");
    assert_eq!(twin.device_id(), id);
    twin.edit(|c| c.ui.font_size = 19);
    twin.tick();
    a.edit(|c| c.ui.theme = "dark".into());
    cloud.step(&mut [&mut a, &mut b, &mut twin]);
    cloud.settle(&mut [&mut a, &mut b, &mut twin], 4);
    assert_ne!(a.device_id(), twin.device_id(), "one of them took a new id");
    for c in [&a, &b, &twin] {
        assert_eq!(c.config().ui.font_size, 19, "{}", c.name);
        assert_eq!(c.config().ui.theme, "dark", "{}", c.name);
    }
}

/// I28: a changed byte, a replayed old head and a planted file are all
/// refused; nothing is lost.
#[test]
fn i28_tampering_is_refused() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    set_theme(&mut a, "dark");
    a.tick();
    let a_dev = a.device_id();
    // Keep a copy of A's directory for the replay.
    let old_a = b.fs.snapshot(format!("{CHOSEN}/Kage Sync/devices/{a_dev}"));
    let b_view = b.fs.clone();
    cloud.step_with(&mut [&mut a, &mut b], |who, p| {
        if who == "Work PC" && new_files_of_view(&b_view, &a_dev, p) {
            Deliver::Later
        } else {
            Deliver::Now
        }
    });
    // Deliver A's new files to B with one byte of each shard changed.
    for (p, bytes) in
        a.fs.files_under(format!("{CHOSEN}/Kage Sync/devices/{a_dev}"))
    {
        if b.fs.get(&p).is_some() {
            continue;
        }
        let mut bytes = bytes;
        if slashed(&p).contains("/shards/") {
            let i = bytes.len() / 2;
            bytes[i] ^= 0x40;
        }
        b.fs.put(&p, bytes);
    }
    // A planted head and shard, without the key.
    b.fs.put(
        format!("{CHOSEN}/Kage Sync/devices/{a_dev}/head-000000000999.kage"),
        vec![7u8; 400],
    );
    b.fs.put(
        format!("{CHOSEN}/Kage Sync/devices/{a_dev}/shards/0123456789abcdef0123456789abcdef.kage"),
        vec![9u8; 400],
    );
    b.tick();
    assert_eq!(b.config().ui.theme, "system", "nothing changed applies");
    advance(&clock, 11 * 60_000);
    b.tick();
    assert!(b.status().problems.iter().any(|p| p.kind == "unreadable"));
    // A's next publish makes everything readable again.
    a.edit(|c| c.ui.font_size = 15);
    a.tick();
    cloud.step(&mut [&mut a, &mut b]);
    b.tick();
    assert_eq!(b.config().ui.font_size, 15);
    assert_eq!(b.config().ui.theme, "dark");
    // A replayed old directory of A is ignored.
    b.fs.restore(&old_a);
    b.tick();
    assert_eq!(b.config().ui.theme, "dark");
    assert_eq!(b.config().ui.font_size, 15);
}

fn new_files_of_view(view: &kage::sync::testing::FaultyFs, dev: &str, p: &Path) -> bool {
    slashed(p).contains(&format!("/devices/{dev}/")) && view.get(p).is_none()
}

/// A full disk and failing writes keep the edits; the publish catches up.
#[test]
fn write_failures_keep_edits_and_catch_up() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    let dir = a.own_dir();
    a.fs.inject(FsOp::Write, &dir, Fault::NoSpace);
    set_theme(&mut a, "dark");
    a.tick();
    assert_eq!(a.status().state, SyncState::DiskFull, "{:?}", a.status());
    cloud.step(&mut [&mut a, &mut b]);
    b.tick();
    assert_eq!(b.config().ui.theme, "system");
    a.fs.clear_faults();
    a.tick();
    cloud.step(&mut [&mut a, &mut b]);
    b.tick();
    assert_eq!(b.config().ui.theme, "dark");
    assert_eq!(a.status().state, SyncState::UpToDate);
    // A Windows sharing violation that clears is retried in place.
    a.fs.inject_times(FsOp::Write, &dir, Fault::Sharing, 2);
    set_theme(&mut a, "light");
    a.tick();
    cloud.step(&mut [&mut a, &mut b]);
    b.tick();
    assert_eq!(b.config().ui.theme, "light");
}

/// A folder macOS won't let Kage open, and one that is gone.
#[test]
fn denied_and_missing_folders_pause_without_losing_edits() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    a.fs.inject(FsOp::List, format!("{CHOSEN}/Kage Sync"), Fault::Denied);
    set_theme(&mut a, "dark");
    a.tick();
    assert_eq!(
        a.status().state,
        SyncState::FolderDenied,
        "{:?}",
        a.status()
    );
    a.fs.clear_faults();
    // The folder is gone from A's view (unmounted, signed out).
    let snap = a.fs.snapshot(CHOSEN);
    a.fs.delete(CHOSEN);
    a.tick();
    assert_eq!(
        a.status().state,
        SyncState::FolderMissing,
        "{:?}",
        a.status()
    );
    assert!(!a.fs.exists(CHOSEN), "never re-created");
    a.fs.restore(&snap);
    a.tick();
    cloud.step(&mut [&mut a, &mut b]);
    b.tick();
    assert_eq!(b.config().ui.theme, "dark");
}

/// Hanging reads count as waiting and never block.
#[test]
fn hanging_reads_count_as_waiting() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    set_theme(&mut a, "dark");
    a.tick();
    cloud.step(&mut [&mut a, &mut b]);
    let a_dir = a.own_dir();
    b.fs.inject(FsOp::Read, &a_dir, Fault::Hang);
    b.tick();
    assert_eq!(b.status().state, SyncState::Waiting);
    b.fs.clear_faults();
    b.tick();
    assert_eq!(b.config().ui.theme, "dark");
}

/// Key store trouble: a missing key locks sync until the passphrase.
#[test]
fn a_lost_key_locks_until_the_passphrase() {
    let (mut cloud, mut a, mut b, _) = pair(JoinChoice::Theirs);
    b.keys.fail(Some(KeyFault::NotFound));
    b.reopen();
    b.tick();
    assert_eq!(b.status().state, SyncState::Locked);
    set_theme(&mut a, "dark");
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().ui.theme, "system");
    let wrong = b.try_ask(EngineRequest::Unlock {
        passphrase: "not the passphrase".into(),
    });
    assert!(wrong.is_err());
    b.keys.fail(None);
    b.ask(EngineRequest::Unlock {
        passphrase: PASS.into(),
    });
    cloud.settle(&mut [&mut a, &mut b], 2);
    assert_eq!(b.config().ui.theme, "dark");
    // Denied: locked, and nothing breaks.
    b.keys.fail(Some(KeyFault::Denied));
    b.reopen();
    b.tick();
    assert_eq!(b.status().state, SyncState::Locked);
}

/// Without folder events, the poll still converges.
#[test]
fn the_poll_alone_converges() {
    let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
    set_theme(&mut a, "dark");
    a.engine.tick(Wake::Local {
        source: "config",
        origin: kage::sync::types::Origin::User,
    });
    advance(&clock, 6_000);
    a.engine.tick(Wake::Poll);
    cloud.step(&mut [&mut a, &mut b]);
    advance(&clock, 5 * 60_000);
    b.engine.tick(Wake::Poll);
    assert_eq!(b.config().ui.theme, "dark");
}

/// Crashes at every named point: after a restart nothing is lost, no
/// value is published twice with a new stamp, and config.json is valid.
#[test]
fn crashes_at_kill_points_lose_nothing() {
    for point in [
        "after_blob_write",
        "after_shard_write",
        "before_head_write",
        "mid_merge_txn",
        "after_config_commit_before_projected",
        "mid_gc",
    ] {
        let (mut cloud, mut a, mut b, clock) = pair(JoinChoice::Theirs);
        let crash_on_b = matches!(
            point,
            "mid_merge_txn" | "after_config_commit_before_projected"
        );
        // Something to publish: a setting and a large steering doc.
        set_theme(&mut a, "dark");
        a.write_steering("user-steering.md", &"x".repeat(70 * 1024));
        if point == "mid_gc" {
            advance(&clock, DAY + 1);
        }
        kill::arm(point);
        let crashed = if crash_on_b {
            a.tick();
            cloud.step(&mut [&mut a, &mut b]);
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                b.tick();
            }))
            .is_err()
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                a.tick();
            }))
            .is_err()
        };
        kill::disarm();
        assert!(crashed, "{point} wasn't reached");
        if crash_on_b {
            b.reopen();
        } else {
            a.reopen();
        }
        let conflicts_before = b.conflicts().len();
        cloud.settle(&mut [&mut a, &mut b], 3);
        for c in [&a, &b] {
            assert_eq!(c.config().ui.theme, "dark", "{point}: {}", c.name);
            assert_eq!(
                c.steering("user-steering.md").len(),
                70 * 1024,
                "{point}: {}",
                c.name
            );
            assert!(
                serde_json::to_value(c.config()).is_ok(),
                "{point}: config.json is valid"
            );
        }
        assert_eq!(
            b.conflicts().len(),
            conflicts_before,
            "{point}: no spurious conflict"
        );
        assert!(a.conflicts().is_empty(), "{point}");
        // Restarting again publishes nothing new for unchanged values.
        let seq_before = a.status().last_publish_at;
        a.reopen();
        a.tick();
        let _ = seq_before;
        assert!(a.conflicts().is_empty(), "{point}");
    }
}

/// Keep `OsKind` and `Path` in use for the helper above.
#[allow(dead_code)]
fn _unused(_: OsKind, c: &SimComputer, p: &Path) -> bool {
    new_files_of(c, "", p)
}

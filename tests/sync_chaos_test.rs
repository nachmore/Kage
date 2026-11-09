//! Seeded chaos (docs/design/SYNC.md §18.4): 3–4 computers on mixed
//! OSes, random edits, deletes, reorders, scope changes, offline periods
//! and a cloud that delays, drops and half-delivers files. After
//! everything settles, every computer agrees.
//!
//! CI mode runs 100 seeds; `KAGE_SYNC_CHAOS_SEEDS=<n>` runs more, and
//! `KAGE_SYNC_CHAOS_SEED=<s>` replays one. Isolates user dirs first.

mod sync_support;

use kage::sync::types::{OsKind, ScopeAction};
use kage::sync::{EngineRequest, JoinChoice};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use sync_support::*;

const THEMES: [&str; 4] = ["dark", "light", "system", "nord"];
const OPS: usize = 32;

fn shortcut(name: &str, url: &str) -> kage::config::ShortcutConfig {
    serde_json::from_value(json!({ "name": name, "action_type": "open_url", "url": url })).unwrap()
}

/// One computer's synced view, for comparing computers of one OS.
/// Theme, font size and shortcuts (name, url) in order.
type View = (String, u8, Vec<(String, String)>);

fn view(c: &SimComputer) -> View {
    let cfg = c.config();
    (
        cfg.ui.theme.clone(),
        cfg.ui.font_size,
        cfg.shortcuts
            .iter()
            .map(|s| (s.name.clone(), s.url.clone().unwrap_or_default()))
            .collect(),
    )
}

fn run(seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let clock = clock();
    let oses = [OsKind::Macos, OsKind::Windows, OsKind::Linux, OsKind::Macos];
    let n = 3 + (seed % 2) as usize;
    let mut comps: Vec<SimComputer> = (0..n)
        .map(|i| SimComputer::new(&format!("C{i}"), oses[i], clock.clone()))
        .collect();
    let mut cloud = SimCloud::new();
    comps[0].create();
    {
        let mut refs: Vec<&mut SimComputer> = comps.iter_mut().collect();
        cloud.step(&mut refs);
    }
    for c in comps.iter_mut().skip(1) {
        c.join(JoinChoice::Theirs);
    }
    let mut offline: BTreeSet<usize> = BTreeSet::new();
    let mut next_name = 0;
    for _ in 0..OPS {
        let i = rng.random_range(0..n);
        match rng.random_range(0..12) {
            0 | 1 => {
                let t = THEMES[rng.random_range(0..THEMES.len())].to_string();
                comps[i].edit(move |c| c.ui.theme = t);
            }
            2 => {
                let f = rng.random_range(11..=20u8);
                comps[i].edit(move |c| c.ui.font_size = f);
            }
            3 | 4 => {
                next_name += 1;
                let name = format!("s{seed}-{next_name}");
                comps[i].edit(move |c| c.shortcuts.push(shortcut(&name, "https://x")));
            }
            5 => {
                let k = rng.random_range(0..8usize);
                comps[i].edit(move |c| {
                    if !c.shortcuts.is_empty() {
                        let k = k % c.shortcuts.len();
                        c.shortcuts.remove(k);
                    }
                });
            }
            6 => {
                let (from, to) = (rng.random_range(0..8usize), rng.random_range(0..8usize));
                comps[i].edit(move |c| {
                    if c.shortcuts.len() > 1 {
                        let n = c.shortcuts.len();
                        let s = c.shortcuts.remove(from % n);
                        c.shortcuts.insert(to % n, s);
                    }
                });
            }
            7 => {
                let k = rng.random_range(0..8usize);
                let url = format!("https://{}", rng.random_range(0..1000));
                comps[i].edit(move |c| {
                    if !c.shortcuts.is_empty() {
                        let n = c.shortcuts.len();
                        c.shortcuts[k % n].url = Some(url);
                    }
                });
            }
            8 => {
                let action = [ScopeAction::Os, ScopeAction::UseShared, ScopeAction::All]
                    [rng.random_range(0..3)];
                let _ = comps[i].try_ask(EngineRequest::SetScope {
                    path: "ui.font_size".into(),
                    action,
                });
            }
            9 => {
                // Go offline or come back.
                if !offline.remove(&i) && offline.len() + 1 < n {
                    offline.insert(i);
                }
            }
            _ => {
                comps[i].tick();
            }
        }
        if rng.random_bool(0.4) {
            let drop_to: Option<usize> = rng.random_bool(0.3).then(|| rng.random_range(0..n));
            let half = rng.random_bool(0.2);
            let names: Vec<String> = comps.iter().map(|c| c.name.clone()).collect();
            let mut refs: Vec<&mut SimComputer> = comps
                .iter_mut()
                .enumerate()
                .filter(|(k, _)| !offline.contains(k))
                .map(|(_, c)| c)
                .collect();
            let seeded = rng.random::<u64>();
            cloud.step_with(&mut refs, |who, p| {
                let h = seeded ^ p.to_string_lossy().len() as u64;
                if drop_to.is_some_and(|d| names[d] == who) && h.is_multiple_of(3) {
                    if half {
                        Deliver::Truncated
                    } else {
                        Deliver::Later
                    }
                } else {
                    Deliver::Now
                }
            });
        }
        advance(&clock, rng.random_range(1_000..60_000));
        if rng.random_bool(0.03) {
            advance(&clock, DAY);
        }
    }
    // Quiescence: everyone online, full delivery, time for debounces and
    // "waiting" files to pass. Bulk deletes are accepted.
    for round in 0..6 {
        for c in comps.iter_mut() {
            c.tick();
            if round == 2 {
                let r = reviews(c);
                for m in r.mass_changes {
                    c.ask(EngineRequest::AnswerMassChange {
                        ns: m.id,
                        accept: true,
                    });
                }
            }
        }
        let mut refs: Vec<&mut SimComputer> = comps.iter_mut().collect();
        cloud.step(&mut refs);
        advance(&clock, 11 * 60_000);
    }
    // Every computer of one OS shows the same; the shared parts agree
    // across OSes; the conflicts lists are the same everywhere.
    let mut by_os: BTreeMap<OsKind, View> = BTreeMap::new();
    let first = view(&comps[0]);
    let mut conflict_ids: Option<BTreeSet<String>> = None;
    for c in comps.iter_mut() {
        let v = view(c);
        assert_eq!(v.0, first.0, "seed {seed}: theme differs on {}", c.name);
        assert_eq!(
            v.2.iter().collect::<BTreeSet<_>>(),
            first.2.iter().collect::<BTreeSet<_>>(),
            "seed {seed}: shortcuts differ on {}",
            c.name
        );
        assert_eq!(
            v.2, first.2,
            "seed {seed}: shortcut order differs on {}",
            c.name
        );
        if let Some(prev) = by_os.get(&c.os) {
            assert_eq!(&v, prev, "seed {seed}: {} differs from its OS", c.name);
        }
        by_os.insert(c.os, v);
        let ids: BTreeSet<String> = c.conflicts().into_iter().map(|x| x.id).collect();
        match &conflict_ids {
            None => conflict_ids = Some(ids),
            Some(prev) => assert_eq!(&ids, prev, "seed {seed}: conflicts differ on {}", c.name),
        }
        // Files per computer directory stay within budget (§5.8).
        let dir = c.own_dir();
        let files = c.fs.files_under(&dir).len();
        assert!(
            files <= 400,
            "seed {seed}: {files} files in {}'s directory",
            c.name
        );
    }
}

#[test]
fn chaos_converges() {
    if let Some(seed) = std::env::var("KAGE_SYNC_CHAOS_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        run(seed);
        return;
    }
    let seeds: u64 = std::env::var("KAGE_SYNC_CHAOS_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    // Seeds are independent: run them on a few threads. A failure names
    // its seed; replay it with KAGE_SYNC_CHAOS_SEED.
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 4) as u64;
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                s.spawn(move || {
                    let mut seed = t;
                    while seed < seeds {
                        run(seed);
                        seed += threads;
                    }
                })
            })
            .collect();
        for h in handles {
            if let Err(e) = h.join() {
                std::panic::resume_unwind(e);
            }
        }
    });
}

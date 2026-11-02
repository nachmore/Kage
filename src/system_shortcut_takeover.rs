//! Let a Kage hotkey replace an OS system shortcut while Kage runs — the
//! headline case being **⌘Space instead of Spotlight** on macOS.
//!
//! # Why this exists
//!
//! A global hotkey registered with `tauri-plugin-global-shortcut` (Carbon
//! `RegisterEventHotKey`) that collides with an *enabled* macOS symbolic
//! hotkey registers without error but never fires — WindowServer hands the
//! keystroke to Spotlight first. Launchers like Alfred/Raycast make users
//! untick the Spotlight shortcut in System Settings by hand. Kage instead
//! disables the colliding symbolic hotkey for the login session (via
//! `crate::os::system_shortcuts`, no TCC permission needed) once the user
//! approves it in the hotkey picker, and re-enables it on quit.
//!
//! # Model
//!
//! * `approved` — ids the user explicitly allowed Kage to take over.
//!   Persisted. Approval alone does nothing: an id is only taken while a
//!   *registered* Kage hotkey actually matches its binding, so moving
//!   Kage off ⌘Space gives Spotlight back immediately.
//! * `owned` — ids Kage has disabled and therefore must re-enable.
//!   Persisted on every change: the symbolic-hotkey state outlives our
//!   process, so after a crash / `kill -9` the next launch's first
//!   reconcile restores anything no longer wanted.
//! * `recording` — Space-bound system shortcuts paused while the hotkey
//!   picker is recording, so pressing ⌘Space reaches the Settings webview
//!   instead of opening Spotlight.
//!
//! `reconcile` drives the OS toward `wanted = hotkey ∪ recording`: release
//! `owned \ wanted`, disable whatever in `wanted` is enabled. A shortcut
//! that is already disabled and not owned was turned off by the user
//! (e.g. a former Raycast setup) — we never adopt it, so we never
//! "restore" something they switched off themselves.
//!
//! The state file lives next to `config.json` and is owned by Rust alone
//! (never round-tripped through the frontend's `save_config`), so a
//! settings save can't clobber approvals.

use crate::os::system_shortcuts::{self, SystemShortcut};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// How often the watchdog re-asserts takeovers. System Settings or another
/// app can re-enable a symbolic hotkey behind our back (e.g. editing any
/// keyboard shortcut re-applies the whole set); without this ⌘Space would
/// silently go back to Spotlight until the next config change.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedState {
    #[serde(default)]
    approved: BTreeSet<u32>,
    #[serde(default)]
    owned: BTreeSet<u32>,
}

#[derive(Default)]
struct Takeover {
    loaded: bool,
    persisted: PersistedState,
    hotkey_wanted: BTreeSet<u32>,
    recording_wanted: BTreeSet<u32>,
    /// Set by `release_all` on shutdown; every later call is a no-op so a
    /// late watchdog tick or config event can't re-disable Spotlight
    /// after we've handed it back.
    released: bool,
    watchdog_started: bool,
}

static STATE: LazyLock<Mutex<Takeover>> = LazyLock::new(|| Mutex::new(Takeover::default()));

/// OS access, abstracted so the reconcile logic is unit-testable.
trait Backend {
    fn get(&self, id: u32) -> Option<SystemShortcut>;
    fn is_enabled(&self, id: u32) -> Option<bool>;
    fn set_enabled(&self, id: u32, enabled: bool) -> bool;
}

struct OsBackend;

impl Backend for OsBackend {
    fn get(&self, id: u32) -> Option<SystemShortcut> {
        system_shortcuts::get(id)
    }
    fn is_enabled(&self, id: u32) -> Option<bool> {
        system_shortcuts::is_enabled(id)
    }
    fn set_enabled(&self, id: u32, enabled: bool) -> bool {
        system_shortcuts::set_enabled(id, enabled)
    }
}

/// A system shortcut that collides with a candidate Kage hotkey.
#[derive(Debug, Clone, Serialize)]
pub struct Conflict {
    pub id: u32,
    /// Localised human name ("Spotlight search").
    pub name: String,
    /// The system shortcut's binding in Kage's `Mods+Key` format.
    pub combo: String,
    /// Already approved for takeover — the picker can skip the prompt.
    pub approved: bool,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Re-point takeovers at the hotkeys Kage has just registered. Called by
/// `register_all_hotkeys` (startup, config changes, end of recording).
pub fn apply_for_hotkeys(hotkeys: &[String]) {
    with_state(|st, be| {
        st.hotkey_wanted = wanted_for_hotkeys(&st.persisted.approved, hotkeys, be);
        reconcile(st, be);
    });
    ensure_watchdog();
}

/// Pause Space-bound system shortcuts (Spotlight, Finder search, input
/// source switching, …) for the duration of a hotkey recording.
pub fn begin_recording() {
    let space_bound: BTreeSet<u32> = system_shortcuts::list()
        .into_iter()
        .filter(|s| s.enabled && s.key.eq_ignore_ascii_case("space"))
        .map(|s| s.id)
        .collect();
    with_state(|st, be| {
        st.recording_wanted = space_bound;
        reconcile(st, be);
    });
}

/// Undo `begin_recording`. Shortcuts that are also wanted by a registered
/// hotkey stay disabled.
pub fn end_recording() {
    with_state(|st, be| {
        st.recording_wanted.clear();
        reconcile(st, be);
    });
}

/// System shortcuts whose binding equals `hotkey`. Includes ones Kage
/// itself currently has disabled (owned), so a recording-paused ⌘Space is
/// still reported.
pub fn conflicts_for(hotkey: &str) -> Vec<Conflict> {
    let all = system_shortcuts::list();
    let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
    ensure_loaded(&mut st);
    all.into_iter()
        .filter(|s| s.enabled || st.persisted.owned.contains(&s.id))
        .filter(|s| combos_match(&s.combo_string(), hotkey))
        .map(|s| Conflict {
            id: s.id,
            name: display_name(s.id),
            combo: s.combo_string(),
            approved: st.persisted.approved.contains(&s.id),
        })
        .collect()
}

/// Record the user's consent to take over `ids`. Unknown ids are ignored.
/// Takes effect at the next `apply_for_hotkeys`.
pub fn approve(ids: &[u32]) {
    with_state(|st, be| {
        let mut changed = false;
        for &id in ids {
            if be.get(id).is_none() {
                warn!("[system-shortcuts] ignoring approval for unknown id {}", id);
                continue;
            }
            changed |= st.persisted.approved.insert(id);
        }
        if changed {
            info!(
                "[system-shortcuts] approved for takeover: {:?}",
                st.persisted.approved
            );
            persist(&st.persisted);
        }
    });
}

/// Give every owned shortcut back to the OS. Called on every shutdown
/// path (graceful_shutdown, RunEvent::Exit, SIGTERM/SIGINT). Idempotent;
/// afterwards the module is inert for the rest of the process.
pub fn release_all() {
    let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
    if st.released {
        return;
    }
    ensure_loaded(&mut st);
    st.released = true;
    st.hotkey_wanted.clear();
    st.recording_wanted.clear();
    if st.persisted.owned.is_empty() {
        return;
    }
    let be = OsBackend;
    release_unwanted(&mut st.persisted, &BTreeSet::new(), &be);
    persist(&st.persisted);
}

/// Localised name of a well-known system shortcut id.
pub fn display_name(id: u32) -> String {
    crate::i18n::translate(display_name_key(id), &[])
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn with_state(f: impl FnOnce(&mut Takeover, &dyn Backend)) {
    let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
    if st.released {
        return;
    }
    ensure_loaded(&mut st);
    f(&mut st, &OsBackend);
}

fn ensure_loaded(st: &mut Takeover) {
    if st.loaded {
        return;
    }
    st.loaded = true;
    st.persisted = load();
    if !st.persisted.owned.is_empty() {
        info!(
            "[system-shortcuts] previous run left {:?} disabled — will reconcile",
            st.persisted.owned
        );
    }
}

fn wanted_for_hotkeys(
    approved: &BTreeSet<u32>,
    hotkeys: &[String],
    be: &dyn Backend,
) -> BTreeSet<u32> {
    approved
        .iter()
        .copied()
        .filter(|&id| {
            be.get(id)
                .map(|s| hotkeys.iter().any(|hk| combos_match(&s.combo_string(), hk)))
                .unwrap_or(false)
        })
        .collect()
}

/// Drive the OS toward `hotkey_wanted ∪ recording_wanted`. Persists `owned`
/// when it changes.
fn reconcile(st: &mut Takeover, be: &dyn Backend) {
    let wanted: BTreeSet<u32> = st
        .hotkey_wanted
        .union(&st.recording_wanted)
        .copied()
        .collect();
    let before = st.persisted.owned.clone();
    release_unwanted(&mut st.persisted, &wanted, be);
    take_wanted(&mut st.persisted, &wanted, be);
    if st.persisted.owned != before {
        persist(&st.persisted);
    }
}

fn release_unwanted(p: &mut PersistedState, wanted: &BTreeSet<u32>, be: &dyn Backend) {
    let to_release: Vec<u32> = p.owned.difference(wanted).copied().collect();
    for id in to_release {
        if be.set_enabled(id, true) {
            info!("[system-shortcuts] restored system shortcut {}", id);
            p.owned.remove(&id);
        } else if be.is_enabled(id).is_none() {
            // The id no longer exists (OS update) — nothing to restore.
            p.owned.remove(&id);
        }
        // Otherwise keep it owned so the next reconcile / launch retries.
    }
}

fn take_wanted(p: &mut PersistedState, wanted: &BTreeSet<u32>, be: &dyn Backend) {
    for &id in wanted {
        // Already disabled means ours (keep owning) or the user's (leave it).
        if be.is_enabled(id) == Some(true) && be.set_enabled(id, false) {
            info!(
                "[system-shortcuts] disabled system shortcut {} for Kage",
                id
            );
            p.owned.insert(id);
        }
    }
}

fn ensure_watchdog() {
    {
        let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
        if st.watchdog_started || st.released || st.hotkey_wanted.is_empty() {
            return;
        }
        st.watchdog_started = true;
    }
    let spawned = std::thread::Builder::new()
        .name("kage-system-shortcuts".into())
        .spawn(|| loop {
            std::thread::sleep(WATCHDOG_INTERVAL);
            let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
            if st.released {
                return;
            }
            if st.hotkey_wanted.is_empty() && st.recording_wanted.is_empty() {
                continue;
            }
            let reenabled: Vec<u32> = st
                .hotkey_wanted
                .union(&st.recording_wanted)
                .copied()
                .filter(|&id| OsBackend.is_enabled(id) == Some(true))
                .collect();
            if !reenabled.is_empty() {
                warn!(
                    "[system-shortcuts] {:?} re-enabled externally — taking back",
                    reenabled
                );
                // Re-enabled behind our back: it's still ours to hold.
                for id in &reenabled {
                    if OsBackend.set_enabled(*id, false) {
                        st.persisted.owned.insert(*id);
                    }
                }
                persist(&st.persisted);
            }
        });
    if let Err(e) = spawned {
        warn!("[system-shortcuts] failed to start watchdog: {}", e);
        let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
        st.watchdog_started = false;
    }
}

/// Same combo? Compared through global-hotkey's own parser so aliases
/// (`Cmd`/`Command`/`Super`, `Option`/`Alt`, `KeyA`/`A`, modifier order)
/// all agree with what actually gets registered.
fn combos_match(a: &str, b: &str) -> bool {
    use tauri_plugin_global_shortcut::Shortcut;
    match (Shortcut::from_str(a), Shortcut::from_str(b)) {
        (Ok(x), Ok(y)) => x.mods == y.mods && x.key == y.key,
        _ => false,
    }
}

fn display_name_key(id: u32) -> &'static str {
    match id {
        64 => "system_shortcuts.name.spotlight",
        65 => "system_shortcuts.name.finder_search",
        60 => "system_shortcuts.name.input_source_previous",
        61 => "system_shortcuts.name.input_source_next",
        1 | 2 => "system_shortcuts.name.app_switcher",
        28..=31 | 184 => "system_shortcuts.name.screenshot",
        32 => "system_shortcuts.name.mission_control",
        33 => "system_shortcuts.name.app_windows",
        36 => "system_shortcuts.name.show_desktop",
        79 | 81 => "system_shortcuts.name.switch_space",
        _ => "system_shortcuts.name.generic",
    }
}

fn state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("kage").join("system-shortcuts.json"))
}

fn load() -> PersistedState {
    let Some(path) = state_path() else {
        return PersistedState::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            warn!(
                "[system-shortcuts] ignoring unreadable {}: {}",
                path.display(),
                e
            );
            PersistedState::default()
        }),
        Err(_) => PersistedState::default(),
    }
}

fn persist(p: &PersistedState) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = match serde_json::to_string_pretty(p) {
        Ok(b) => b,
        Err(e) => {
            warn!("[system-shortcuts] serialize failed: {}", e);
            return;
        }
    };
    // Write-then-rename so a crash mid-write can't lose the `owned` list —
    // that list is the only record of what to restore.
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, &path)) {
        warn!(
            "[system-shortcuts] persist to {} failed: {}",
            path.display(),
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// In-memory stand-in for WindowServer's symbolic-hotkey table.
    struct Fake {
        table: RefCell<BTreeMap<u32, SystemShortcut>>,
        fail_set: BTreeSet<u32>,
    }

    impl Fake {
        fn new(entries: &[(u32, &str, &str, bool)]) -> Self {
            let table = entries
                .iter()
                .map(|(id, mods, key, enabled)| {
                    let modifiers = if mods.is_empty() {
                        vec![]
                    } else {
                        mods.split('+').map(String::from).collect()
                    };
                    (
                        *id,
                        SystemShortcut {
                            id: *id,
                            enabled: *enabled,
                            modifiers,
                            key: key.to_string(),
                        },
                    )
                })
                .collect();
            Self {
                table: RefCell::new(table),
                fail_set: BTreeSet::new(),
            }
        }
        fn enabled(&self, id: u32) -> bool {
            self.table.borrow()[&id].enabled
        }
    }

    impl Backend for Fake {
        fn get(&self, id: u32) -> Option<SystemShortcut> {
            self.table.borrow().get(&id).cloned()
        }
        fn is_enabled(&self, id: u32) -> Option<bool> {
            self.table.borrow().get(&id).map(|s| s.enabled)
        }
        fn set_enabled(&self, id: u32, enabled: bool) -> bool {
            if self.fail_set.contains(&id) {
                return false;
            }
            match self.table.borrow_mut().get_mut(&id) {
                Some(s) => {
                    s.enabled = enabled;
                    true
                }
                None => false,
            }
        }
    }

    fn stock() -> Fake {
        Fake::new(&[
            (64, "Super", "Space", true),     // Spotlight
            (65, "Alt+Super", "Space", true), // Finder search
            (60, "Ctrl", "Space", false),     // input source — user disabled
            (32, "Ctrl", "Up", true),         // Mission Control
        ])
    }

    fn set(ids: &[u32]) -> BTreeSet<u32> {
        ids.iter().copied().collect()
    }

    fn hk(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // reconcile() persists to the real config dir; the tests drive the
    // pure helpers instead.
    fn run(p: &mut PersistedState, wanted: &BTreeSet<u32>, be: &dyn Backend) {
        release_unwanted(p, wanted, be);
        take_wanted(p, wanted, be);
    }

    #[test]
    fn combos_match_handles_aliases_and_order() {
        assert!(combos_match("Super+Space", "Cmd+Space"));
        assert!(combos_match("Super+Space", "command+space"));
        assert!(combos_match("Alt+Super+Space", "Super+Alt+Space"));
        assert!(combos_match("Ctrl+Shift+KeyA", "Shift+Ctrl+A"));
        assert!(!combos_match("Super+Space", "Alt+Space"));
        assert!(!combos_match("Super+Space", "Super+Shift+Space"));
        assert!(!combos_match("garbage+", "Super+Space"));
    }

    #[test]
    fn approval_alone_takes_nothing() {
        let be = stock();
        let wanted = wanted_for_hotkeys(&set(&[64]), &hk(&["Alt+Space"]), &be);
        assert!(wanted.is_empty());
    }

    #[test]
    fn matching_registered_hotkey_takes_approved_shortcut_only() {
        let be = stock();
        // Kage on ⌘Space, but only Spotlight approved — Finder search (⌥⌘Space)
        // is a different combo and Mission Control isn't approved.
        let wanted = wanted_for_hotkeys(&set(&[64, 32]), &hk(&["Super+Space"]), &be);
        assert_eq!(wanted, set(&[64]));
        let mut p = PersistedState::default();
        run(&mut p, &wanted, &be);
        assert!(!be.enabled(64));
        assert_eq!(p.owned, set(&[64]));
        assert!(be.enabled(65));
    }

    #[test]
    fn unapproved_conflict_is_never_taken() {
        let be = stock();
        let wanted = wanted_for_hotkeys(&BTreeSet::new(), &hk(&["Super+Space"]), &be);
        assert!(wanted.is_empty());
    }

    #[test]
    fn moving_off_the_combo_restores_it() {
        let be = stock();
        let mut p = PersistedState::default();
        run(&mut p, &set(&[64]), &be);
        assert!(!be.enabled(64));
        run(&mut p, &BTreeSet::new(), &be);
        assert!(be.enabled(64));
        assert!(p.owned.is_empty());
    }

    #[test]
    fn user_disabled_shortcut_is_never_adopted_or_restored() {
        let be = stock();
        let mut p = PersistedState::default();
        // Ctrl+Space (60) is already off by the user's choice.
        run(&mut p, &set(&[60]), &be);
        assert!(
            p.owned.is_empty(),
            "must not adopt a user-disabled shortcut"
        );
        run(&mut p, &BTreeSet::new(), &be);
        assert!(
            !be.enabled(60),
            "must not re-enable what the user turned off"
        );
    }

    #[test]
    fn crash_recovery_restores_stale_owned_ids() {
        // A previous run disabled Spotlight and died. The table still shows
        // it disabled; on this launch Kage's hotkey is no longer ⌘Space.
        let be = stock();
        be.set_enabled(64, false);
        let mut p = PersistedState {
            approved: set(&[64]),
            owned: set(&[64]),
        };
        let wanted = wanted_for_hotkeys(&p.approved, &hk(&["Alt+Space"]), &be);
        run(&mut p, &wanted, &be);
        assert!(be.enabled(64));
        assert!(p.owned.is_empty());
    }

    #[test]
    fn crash_recovery_keeps_still_wanted_ids_owned() {
        let be = stock();
        be.set_enabled(64, false);
        let mut p = PersistedState {
            approved: set(&[64]),
            owned: set(&[64]),
        };
        let wanted = wanted_for_hotkeys(&p.approved, &hk(&["Super+Space"]), &be);
        run(&mut p, &wanted, &be);
        assert!(!be.enabled(64));
        assert_eq!(p.owned, set(&[64]), "still ours — must restore on quit");
    }

    #[test]
    fn failed_restore_stays_owned_for_retry() {
        let mut be = stock();
        let mut p = PersistedState::default();
        run(&mut p, &set(&[64]), &be);
        be.fail_set.insert(64);
        run(&mut p, &BTreeSet::new(), &be);
        assert_eq!(p.owned, set(&[64]));
    }

    #[test]
    fn vanished_id_is_dropped_from_owned() {
        let be = stock();
        let mut p = PersistedState {
            approved: BTreeSet::new(),
            owned: set(&[999]),
        };
        run(&mut p, &BTreeSet::new(), &be);
        assert!(p.owned.is_empty());
    }

    #[test]
    fn persisted_state_tolerates_missing_fields() {
        let p: PersistedState = serde_json::from_str("{}").unwrap();
        assert_eq!(p, PersistedState::default());
        let p: PersistedState = serde_json::from_str(r#"{"owned":[64]}"#).unwrap();
        assert_eq!(p.owned, set(&[64]));
    }

    #[test]
    fn every_display_name_key_exists_in_en_catalog() {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../locales/en/messages.json")).unwrap();
        for id in [64, 65, 60, 61, 1, 2, 28, 184, 32, 33, 36, 79, 81, 12345] {
            let key = display_name_key(id);
            assert!(
                catalog.get(key).is_some(),
                "missing EN catalog key {} (id {})",
                key,
                id
            );
        }
    }
}

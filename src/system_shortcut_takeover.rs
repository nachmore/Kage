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
//! * `approved` — `id → combo` the user explicitly allowed Kage to take
//!   over. Keyed by the binding at approval time: if the user later moves
//!   Spotlight to another combo, that approval no longer applies and the
//!   picker asks again. Approval alone does nothing — an id is only taken
//!   while a *registered* Kage hotkey matches its *current* binding, so
//!   moving Kage off ⌘Space gives Spotlight back immediately.
//! * `owned` — ids Kage has disabled and therefore must re-enable. Written
//!   *before* each disable (write-ahead): the symbolic-hotkey state
//!   outlives our process, so after a crash / `kill -9` the next launch's
//!   first reconcile must know what to restore.
//! * `recording` — Space-bound system shortcuts paused while a hotkey
//!   picker is recording, so pressing ⌘Space reaches the Settings webview
//!   instead of opening Spotlight.
//!
//! `reconcile` drives the OS toward `wanted = hotkey ∪ recording`: release
//! `owned \ wanted`, disable whatever in `wanted` is enabled. Two rules
//! protect the user's own choices:
//!
//! * A shortcut that is already disabled and not owned was turned off by
//!   the user (e.g. a former Raycast setup) — never adopted.
//! * Before re-enabling an owned shortcut we check the user's *saved*
//!   preference (what System Settings shows); if they unticked it while
//!   Kage held it, we drop it instead of turning it back on.
//!
//! A watchdog re-runs the whole computation every few seconds (System
//! Settings re-applies every shortcut when any one is edited), so a
//! rebound or re-enabled shortcut is either released or re-taken based on
//! its live binding — never blindly by id.
//!
//! The state file lives next to `config.json` and is owned by Rust alone
//! (never round-tripped through the frontend's `save_config`), so a
//! settings save can't clobber approvals.

use crate::os::system_shortcuts::{self, SystemShortcut};
use log::{info, warn};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// How often the watchdog re-evaluates takeovers.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PersistedState {
    /// id → combo string (Kage `Mods+Key` format) approved for takeover.
    approved: BTreeMap<u32, String>,
    owned: BTreeSet<u32>,
}

impl PersistedState {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "approved": self.approved.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>(),
            "owned": self.owned,
        })
    }

    /// Lenient: a malformed `approved` must never cost us the `owned` list
    /// (the only record of what to restore), and vice versa.
    fn from_json(v: &serde_json::Value) -> Self {
        let owned = v
            .get("owned")
            .and_then(|o| o.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
                    .collect()
            })
            .unwrap_or_default();
        let approved = v
            .get("approved")
            .and_then(|o| o.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, combo)| Some((k.parse().ok()?, combo.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        Self { approved, owned }
    }
}

#[derive(Default)]
struct Takeover {
    loaded: bool,
    persisted: PersistedState,
    /// Hotkeys that registered in the last `register_all_hotkeys` pass.
    hotkeys: Vec<String>,
    recording_wanted: BTreeSet<u32>,
    /// Set by `release_all` on shutdown: every later call is a no-op so a
    /// late watchdog tick or config event can't re-disable Spotlight after
    /// we've handed it back. `resume` clears it (failed update install).
    released: bool,
    watchdog_started: bool,
}

static STATE: LazyLock<Mutex<Takeover>> = LazyLock::new(|| Mutex::new(Takeover::default()));

/// OS + storage access, abstracted so the reconcile logic is unit-testable.
trait Backend {
    fn get(&self, id: u32) -> Option<SystemShortcut>;
    fn is_enabled(&self, id: u32) -> Option<bool>;
    fn set_enabled(&self, id: u32, enabled: bool) -> bool;
    /// `Some(false)` when the user switched the shortcut off in System
    /// Settings (their saved preference), regardless of live state.
    fn user_preference(&self, id: u32) -> Option<bool>;
    /// Durably record `p`. Returns false if it could not be written.
    fn persist(&self, p: &PersistedState) -> bool;
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
    fn user_preference(&self, id: u32) -> Option<bool> {
        system_shortcuts::user_preference(id)
    }
    fn persist(&self, p: &PersistedState) -> bool {
        persist_to_disk(p)
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
    /// Already approved for takeover *on this binding* — the picker can
    /// skip the prompt.
    pub approved: bool,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Re-point takeovers at the hotkeys Kage has just registered. Called by
/// `register_all_hotkeys` (startup, config changes, end of recording).
pub fn apply_for_hotkeys(hotkeys: &[String]) {
    with_state(|st, be| {
        st.hotkeys = hotkeys.to_vec();
        reconcile(st, be);
    });
    ensure_watchdog();
}

/// Pause Space-bound system shortcuts (Spotlight, Finder search, input
/// source switching, …) while a hotkey picker records. Additive: an
/// overlapping second recording keeps what the first one paused (those
/// ids now read as disabled, so they're picked up through `owned`).
pub fn begin_recording() {
    let all = system_shortcuts::list();
    with_state(|st, be| {
        let space_bound: Vec<u32> = all
            .iter()
            .filter(|s| s.key.eq_ignore_ascii_case("space"))
            .filter(|s| s.enabled || st.persisted.owned.contains(&s.id))
            .filter(|s| be.user_preference(s.id) != Some(false))
            .map(|s| s.id)
            .collect();
        st.recording_wanted.extend(space_bound);
        reconcile(st, be);
    });
}

/// Undo `begin_recording`. Shortcuts also wanted by a registered hotkey
/// stay disabled. (Overlapping recordings are serialised by the command
/// layer's generation check, so clearing everything here is correct.)
pub fn end_recording() {
    with_state(|st, be| {
        st.recording_wanted.clear();
        reconcile(st, be);
    });
}

/// System shortcuts whose binding equals `hotkey` and that would swallow
/// it: enabled ones, plus ones Kage itself currently holds (owned) unless
/// the user has since switched them off.
pub fn conflicts_for(hotkey: &str) -> Vec<Conflict> {
    let all = system_shortcuts::list();
    let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
    ensure_loaded(&mut st, &OsBackend);
    let be = OsBackend;
    all.into_iter()
        .filter(|s| s.enabled || st.persisted.owned.contains(&s.id))
        .filter(|s| be.user_preference(s.id) != Some(false))
        .filter(|s| combos_match(&s.combo_string(), hotkey))
        .map(|s| Conflict {
            id: s.id,
            name: display_name(s.id),
            combo: s.combo_string(),
            approved: is_approved_for(&st.persisted, &s),
        })
        .collect()
}

/// Record the user's consent to take over `ids` on their *current*
/// bindings. Unknown ids are ignored. Takes effect at the next
/// `apply_for_hotkeys`.
pub fn approve(ids: &[u32]) {
    with_state(|st, be| {
        let mut changed = false;
        for &id in ids {
            let Some(sc) = be.get(id) else {
                warn!("[system-shortcuts] ignoring approval for unknown id {}", id);
                continue;
            };
            let combo = sc.combo_string();
            if st.persisted.approved.get(&id) != Some(&combo) {
                st.persisted.approved.insert(id, combo);
                changed = true;
            }
        }
        if changed {
            info!(
                "[system-shortcuts] approved for takeover: {:?}",
                st.persisted.approved
            );
            be.persist(&st.persisted);
        }
    });
}

/// Give every owned shortcut back to the OS. Called on every shutdown
/// path (graceful_shutdown, RunEvent::Exit, SIGTERM/SIGINT). Idempotent;
/// afterwards the module is inert until `resume`.
pub fn release_all() {
    let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
    if st.released {
        return;
    }
    let be = OsBackend;
    ensure_loaded(&mut st, &be);
    st.released = true;
    st.recording_wanted.clear();
    if st.persisted.owned.is_empty() {
        return;
    }
    release_unwanted(&mut st.persisted, &BTreeSet::new(), &be);
    be.persist(&st.persisted);
}

/// Undo `release_all` when shutdown turned out not to happen (an update
/// install that failed after teardown). The caller re-runs
/// `register_all_hotkeys`, which re-applies takeovers.
pub fn resume() {
    let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
    if st.released {
        info!("[system-shortcuts] resuming after aborted shutdown");
        st.released = false;
    }
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
    let be = OsBackend;
    ensure_loaded(&mut st, &be);
    f(&mut st, &be);
}

fn ensure_loaded(st: &mut Takeover, _be: &dyn Backend) {
    if st.loaded {
        return;
    }
    st.loaded = true;
    st.persisted = load_from_disk();
    if !st.persisted.owned.is_empty() {
        info!(
            "[system-shortcuts] previous run left {:?} disabled — will reconcile",
            st.persisted.owned
        );
    }
}

/// Approved *and* still bound to the combo the user approved.
fn is_approved_for(p: &PersistedState, sc: &SystemShortcut) -> bool {
    p.approved
        .get(&sc.id)
        .is_some_and(|combo| combos_match(combo, &sc.combo_string()))
}

fn wanted_for_hotkeys(p: &PersistedState, hotkeys: &[String], be: &dyn Backend) -> BTreeSet<u32> {
    p.approved
        .keys()
        .copied()
        .filter(|&id| {
            be.get(id).is_some_and(|s| {
                is_approved_for(p, &s)
                    && be.user_preference(id) != Some(false)
                    && hotkeys.iter().any(|hk| combos_match(&s.combo_string(), hk))
            })
        })
        .collect()
}

/// Recompute what's wanted from the live bindings and drive the OS there.
fn reconcile(st: &mut Takeover, be: &dyn Backend) {
    let mut wanted = wanted_for_hotkeys(&st.persisted, &st.hotkeys, be);
    wanted.extend(st.recording_wanted.iter().copied());
    let before = st.persisted.owned.clone();
    release_unwanted(&mut st.persisted, &wanted, be);
    take_wanted(&mut st.persisted, &wanted, be);
    if st.persisted.owned != before {
        be.persist(&st.persisted);
    }
}

fn release_unwanted(p: &mut PersistedState, wanted: &BTreeSet<u32>, be: &dyn Backend) {
    let to_release: Vec<u32> = p.owned.difference(wanted).copied().collect();
    for id in to_release {
        if be.user_preference(id) == Some(false) {
            // The user unticked it in System Settings while we held it —
            // leave it off and stop owning it.
            info!(
                "[system-shortcuts] {} is switched off in the user's settings — not restoring",
                id
            );
            p.owned.remove(&id);
        } else if be.set_enabled(id, true) {
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
        if be.is_enabled(id) != Some(true) {
            continue;
        }
        // Write-ahead: record ownership durably BEFORE disabling, so a
        // crash between the two can't strand the shortcut as "user
        // disabled" forever.
        let newly_owned = p.owned.insert(id);
        if newly_owned && !be.persist(p) {
            warn!(
                "[system-shortcuts] can't persist ownership of {} — not disabling it",
                id
            );
            p.owned.remove(&id);
            continue;
        }
        if be.set_enabled(id, false) {
            info!(
                "[system-shortcuts] disabled system shortcut {} for Kage",
                id
            );
        } else if newly_owned {
            p.owned.remove(&id);
        }
    }
}

fn ensure_watchdog() {
    {
        let mut st = STATE.lock().unwrap_or_else(|p| p.into_inner());
        if st.watchdog_started || st.released || st.persisted.approved.is_empty() {
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
                st.watchdog_started = false;
                return;
            }
            // Full recompute from live bindings + saved preferences: a
            // shortcut the user rebound or switched off is released; one
            // re-enabled behind our back on Kage's combo is taken back.
            reconcile(&mut st, &OsBackend);
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

fn load_from_disk() -> PersistedState {
    let Some(path) = state_path() else {
        return PersistedState::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<serde_json::Value>(&s) {
            Ok(v) => PersistedState::from_json(&v),
            Err(e) => {
                warn!(
                    "[system-shortcuts] ignoring unreadable {}: {}",
                    path.display(),
                    e
                );
                PersistedState::default()
            }
        },
        Err(_) => PersistedState::default(),
    }
}

fn persist_to_disk(p: &PersistedState) -> bool {
    let Some(path) = state_path() else {
        return false;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = match serde_json::to_string_pretty(&p.to_json()) {
        Ok(b) => b,
        Err(e) => {
            warn!("[system-shortcuts] serialize failed: {}", e);
            return false;
        }
    };
    // Write-then-rename so a crash mid-write can't lose the `owned` list.
    let tmp = path.with_extension("json.tmp");
    match std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, &path)) {
        Ok(()) => true,
        Err(e) => {
            warn!(
                "[system-shortcuts] persist to {} failed: {}",
                path.display(),
                e
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// In-memory stand-in for WindowServer's symbolic-hotkey table plus the
    /// user's saved preferences and the state file.
    struct Fake {
        table: RefCell<BTreeMap<u32, SystemShortcut>>,
        prefs: RefCell<BTreeMap<u32, bool>>,
        fail_set: BTreeSet<u32>,
        persist_ok: Cell<bool>,
        persisted: RefCell<Vec<PersistedState>>,
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
                prefs: RefCell::new(BTreeMap::new()),
                fail_set: BTreeSet::new(),
                persist_ok: Cell::new(true),
                persisted: RefCell::new(Vec::new()),
            }
        }
        fn enabled(&self, id: u32) -> bool {
            self.table.borrow()[&id].enabled
        }
        fn rebind(&self, id: u32, mods: &[&str], key: &str) {
            let mut t = self.table.borrow_mut();
            let s = t.get_mut(&id).unwrap();
            s.modifiers = mods.iter().map(|m| m.to_string()).collect();
            s.key = key.into();
        }
        fn last_persisted(&self) -> Option<PersistedState> {
            self.persisted.borrow().last().cloned()
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
        fn user_preference(&self, id: u32) -> Option<bool> {
            self.prefs.borrow().get(&id).copied()
        }
        fn persist(&self, p: &PersistedState) -> bool {
            if self.persist_ok.get() {
                self.persisted.borrow_mut().push(p.clone());
                true
            } else {
                false
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

    fn takeover(approved: &[(u32, &str)], hotkeys: &[&str]) -> Takeover {
        Takeover {
            loaded: true,
            persisted: PersistedState {
                approved: approved.iter().map(|(i, c)| (*i, c.to_string())).collect(),
                owned: BTreeSet::new(),
            },
            hotkeys: hk(hotkeys),
            ..Default::default()
        }
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
        let mut st = takeover(&[(64, "Super+Space")], &["Alt+Space"]);
        reconcile(&mut st, &be);
        assert!(be.enabled(64));
        assert!(st.persisted.owned.is_empty());
    }

    #[test]
    fn matching_registered_hotkey_takes_approved_shortcut_only() {
        let be = stock();
        let mut st = takeover(&[(64, "Super+Space"), (32, "Ctrl+Up")], &["Super+Space"]);
        reconcile(&mut st, &be);
        assert!(!be.enabled(64));
        assert_eq!(st.persisted.owned, set(&[64]));
        assert!(be.enabled(65), "different combo");
        assert!(be.enabled(32), "approved but no Kage hotkey on it");
    }

    #[test]
    fn unapproved_conflict_is_never_taken() {
        let be = stock();
        let mut st = takeover(&[], &["Super+Space"]);
        reconcile(&mut st, &be);
        assert!(be.enabled(64));
    }

    #[test]
    fn moving_off_the_combo_restores_it() {
        let be = stock();
        let mut st = takeover(&[(64, "Super+Space")], &["Super+Space"]);
        reconcile(&mut st, &be);
        assert!(!be.enabled(64));
        st.hotkeys = hk(&["Alt+Space"]);
        reconcile(&mut st, &be);
        assert!(be.enabled(64));
        assert!(st.persisted.owned.is_empty());
    }

    #[test]
    fn ownership_is_written_before_the_disable() {
        let be = stock();
        let mut st = takeover(&[(64, "Super+Space")], &["Super+Space"]);
        reconcile(&mut st, &be);
        let first = be.persisted.borrow().first().cloned().unwrap();
        assert!(first.owned.contains(&64), "owned must be durable first");
    }

    #[test]
    fn persist_failure_means_no_disable() {
        let be = stock();
        be.persist_ok.set(false);
        let mut st = takeover(&[(64, "Super+Space")], &["Super+Space"]);
        reconcile(&mut st, &be);
        assert!(be.enabled(64), "never disable what we can't record");
        assert!(st.persisted.owned.is_empty());
    }

    #[test]
    fn user_disabled_shortcut_is_never_adopted_or_restored() {
        let be = stock();
        let mut st = takeover(&[], &[]);
        st.recording_wanted = set(&[60]);
        reconcile(&mut st, &be);
        assert!(
            st.persisted.owned.is_empty(),
            "must not adopt a user-disabled shortcut"
        );
        st.recording_wanted.clear();
        reconcile(&mut st, &be);
        assert!(!be.enabled(60));
    }

    #[test]
    fn user_unticking_while_owned_is_respected_on_release() {
        let be = stock();
        let mut st = takeover(&[(64, "Super+Space")], &["Super+Space"]);
        reconcile(&mut st, &be);
        assert!(!be.enabled(64));
        // User moves to Raycast: unticks Spotlight in System Settings.
        be.prefs.borrow_mut().insert(64, false);
        st.hotkeys = hk(&["Alt+Space"]);
        reconcile(&mut st, &be);
        assert!(
            !be.enabled(64),
            "must not turn back on what the user turned off"
        );
        assert!(st.persisted.owned.is_empty());
    }

    #[test]
    fn rebinding_spotlight_releases_it_even_while_kage_holds_cmd_space() {
        // The headline watchdog bug: user moves Spotlight to ⌥Space so they
        // can keep both. System Settings re-enables it on the new binding.
        let be = stock();
        let mut st = takeover(&[(64, "Super+Space")], &["Super+Space"]);
        reconcile(&mut st, &be);
        be.rebind(64, &["Alt"], "Space");
        be.set_enabled(64, true);
        reconcile(&mut st, &be); // watchdog tick
        assert!(
            be.enabled(64),
            "⌥Space Spotlight no longer collides — leave it on"
        );
        assert!(st.persisted.owned.is_empty());
    }

    #[test]
    fn approval_does_not_follow_a_rebind_to_another_kage_hotkey() {
        // Approved ⌘Space; user rebinds Spotlight to ⌃Space and later puts a
        // Kage slot on ⌃Space — they never approved giving up ⌃Space.
        let be = stock();
        let mut st = takeover(&[(64, "Super+Space")], &["Ctrl+Space"]);
        be.rebind(64, &["Ctrl"], "Space");
        reconcile(&mut st, &be);
        assert!(be.enabled(64));
        let sc = be.get(64).unwrap();
        assert!(
            !is_approved_for(&st.persisted, &sc),
            "picker must ask again"
        );
    }

    #[test]
    fn re_enabled_on_kages_combo_is_taken_back() {
        let be = stock();
        let mut st = takeover(&[(64, "Super+Space")], &["Super+Space"]);
        reconcile(&mut st, &be);
        be.set_enabled(64, true); // something re-applied the shortcut set
        reconcile(&mut st, &be); // watchdog tick
        assert!(!be.enabled(64));
        assert_eq!(st.persisted.owned, set(&[64]));
    }

    #[test]
    fn overlapping_recordings_keep_the_pause() {
        let be = stock();
        let mut st = takeover(&[], &[]);
        st.recording_wanted.extend([64, 65]);
        reconcile(&mut st, &be);
        // A second begin sees 64/65 as disabled; it must add, not replace.
        st.recording_wanted.extend(std::iter::empty::<u32>());
        reconcile(&mut st, &be);
        assert!(!be.enabled(64) && !be.enabled(65));
    }

    #[test]
    fn crash_recovery_restores_stale_owned_ids() {
        let be = stock();
        be.set_enabled(64, false);
        let mut st = takeover(&[(64, "Super+Space")], &["Alt+Space"]);
        st.persisted.owned = set(&[64]);
        reconcile(&mut st, &be);
        assert!(be.enabled(64));
        assert!(st.persisted.owned.is_empty());
    }

    #[test]
    fn crash_recovery_keeps_still_wanted_ids_owned() {
        let be = stock();
        be.set_enabled(64, false);
        let mut st = takeover(&[(64, "Super+Space")], &["Super+Space"]);
        st.persisted.owned = set(&[64]);
        reconcile(&mut st, &be);
        assert!(!be.enabled(64));
        assert_eq!(
            st.persisted.owned,
            set(&[64]),
            "still ours — must restore on quit"
        );
    }

    #[test]
    fn failed_restore_stays_owned_for_retry() {
        let mut be = stock();
        let mut st = takeover(&[(64, "Super+Space")], &["Super+Space"]);
        reconcile(&mut st, &be);
        be.fail_set.insert(64);
        st.hotkeys.clear();
        reconcile(&mut st, &be);
        assert_eq!(st.persisted.owned, set(&[64]));
    }

    #[test]
    fn vanished_id_is_dropped_from_owned() {
        let be = stock();
        let mut st = takeover(&[], &[]);
        st.persisted.owned = set(&[999]);
        reconcile(&mut st, &be);
        assert!(st.persisted.owned.is_empty());
        assert_eq!(be.last_persisted().unwrap().owned, BTreeSet::new());
    }

    #[test]
    fn persisted_state_round_trips_and_tolerates_damage() {
        let p = PersistedState {
            approved: [(64, "Super+Space".to_string())].into_iter().collect(),
            owned: set(&[64]),
        };
        assert_eq!(PersistedState::from_json(&p.to_json()), p);
        assert_eq!(
            PersistedState::from_json(&serde_json::json!({})),
            PersistedState::default()
        );
        // A malformed `approved` (e.g. an older array format) must not cost
        // us the `owned` list.
        let v = serde_json::json!({ "approved": [64], "owned": [64, "x", 70000000000u64] });
        let got = PersistedState::from_json(&v);
        assert_eq!(got.owned, set(&[64]));
        assert!(got.approved.is_empty());
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

//! The view windows that exist, visible or cached hidden
//! (docs/design/EXTENSION_VIEWS.md §7.3, §16.2).
//!
//! Pure bookkeeping: no Tauri types, so every rule here is unit-tested.
//! `crate::ext_views` drives it from the commands and window events.
//!
//! Per label it keeps the boot record (extension, view, params, open
//! sequence, source, booted), whether the window is visible and focused,
//! the teardown token of a hidden window, the geometry save debounce and
//! the compose / copy rate limits.
//!
//! A window being destroyed leaves the record at once and is marked
//! "dying" until its `Destroyed` event arrives, so a late event can't
//! remove the record of a new window with the same label, and an open
//! waits for the old window to go before it builds a new one.

use crate::ext_views::ExtViewInfo;
use crate::lock_ext::LockExt;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// Most view windows at once, visible or hidden.
pub const MAX_WINDOWS: usize = 4;
/// Most hidden (cached) view windows.
pub const MAX_HIDDEN: usize = 2;
/// Window of the compose and copy rate limits.
pub const RATE_WINDOW: Duration = Duration::from_secs(1);
/// Most compose (or copy) requests per [`RATE_WINDOW`], per window.
pub const RATE_MAX: usize = 2;
/// How long a dying window may take to go before an open gives up on it.
pub const DYING_TIMEOUT: Duration = Duration::from_secs(2);

/// What a rate limit counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateKind {
    Compose,
    Copy,
}

/// Every window is visible and the cap is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyOpen;

/// A window that existed and was opened again.
#[derive(Debug, Clone, PartialEq)]
pub struct Reopened {
    pub open_seq: u64,
    /// The view page asked for its boot record already, so it needs an
    /// `ext_view_reopen` event to see the new params.
    pub booted: bool,
    /// It was hidden, so the page needs `ext_view_visibility { visible: true }`.
    pub was_hidden: bool,
}

/// What `ext_view_boot` returns, before the label checks.
#[derive(Debug, Clone, PartialEq)]
pub struct BootRecord {
    pub extension_id: String,
    pub view_id: String,
    pub params: Option<serde_json::Value>,
    pub open_seq: u64,
    pub source: String,
    /// The page booted before in this window.
    pub reloaded: bool,
}

/// A window was hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hidden {
    /// Teardown token: the timer destroys the window only if this is
    /// still the token when it fires.
    pub token: u64,
    /// Hidden windows over [`MAX_HIDDEN`], least recently hidden first.
    /// Their records are gone already; the caller destroys the windows.
    pub evict: Vec<String>,
}

/// Where a geometry debounce stands after its wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// Another move or resize came in; wait again for this token.
    Changed(u64),
    /// Quiet: save now (the window is visible).
    Save,
    /// Quiet, but the window is hidden or gone: don't save.
    Skip,
}

#[derive(Debug)]
struct Record {
    extension_id: String,
    view_id: String,
    params: Option<serde_json::Value>,
    open_seq: u64,
    source: String,
    booted: bool,
    visible: bool,
    focused: bool,
    /// Teardown token; changes on every hide and show.
    hidden_token: u64,
    /// Order of the last hide (higher is more recent).
    hidden_seq: u64,
    last_hidden_at: Option<Instant>,
    geometry_token: u64,
    geometry_pending: bool,
    compose_times: VecDeque<Instant>,
    copy_times: VecDeque<Instant>,
}

#[derive(Debug)]
struct Dying {
    since: Instant,
    /// The window was visible when it was destroyed, so the activation
    /// policy needs a refresh once it's gone.
    was_visible: bool,
}

#[derive(Debug, Default)]
struct Inner {
    records: HashMap<String, Record>,
    dying: HashMap<String, Dying>,
    counter: u64,
}

impl Inner {
    fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// Remove a record and mark its window dying.
    fn take(&mut self, label: &str, now: Instant, refresh: bool) -> Option<Record> {
        let record = self.records.remove(label)?;
        self.dying.insert(
            label.to_string(),
            Dying {
                since: now,
                was_visible: refresh && record.visible,
            },
        );
        Some(record)
    }

    /// The least recently hidden window, if any is hidden.
    fn least_recently_hidden(&self) -> Option<String> {
        self.records
            .iter()
            .filter(|(_, r)| !r.visible)
            .min_by_key(|(_, r)| r.hidden_seq)
            .map(|(label, _)| label.clone())
    }

    fn hidden_count(&self) -> usize {
        self.records.values().filter(|r| !r.visible).count()
    }
}

/// Open view windows by label (§16.2). Held in `UiState.ext_views`.
#[derive(Debug, Default)]
pub struct Registry {
    inner: Mutex<Inner>,
    /// Serialises `ext_view_open`, so two opens of one view can't both
    /// build a window.
    open_lock: tokio::sync::Mutex<()>,
    /// Woken whenever a view window's focus changes.
    focus_changed: Notify,
}

impl Registry {
    /// Held for the whole of an `ext_view_open`.
    pub async fn open_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.open_lock.lock().await
    }

    /// Whether a record exists for `label`.
    pub fn contains(&self, label: &str) -> bool {
        self.inner.lock_or_recover().records.contains_key(label)
    }

    /// Whether the window of `label` is being destroyed. A dying entry
    /// older than [`DYING_TIMEOUT`] no longer counts.
    pub fn is_dying(&self, label: &str, now: Instant) -> bool {
        self.inner
            .lock_or_recover()
            .dying
            .get(label)
            .is_some_and(|d| now.duration_since(d.since) < DYING_TIMEOUT)
    }

    /// Record a new window, visible, before it is built. When the cap is
    /// reached, the least recently hidden windows make room: their
    /// records are removed and their labels returned for the caller to
    /// destroy. With every window visible: [`TooManyOpen`].
    pub fn reserve(
        &self,
        label: &str,
        extension_id: &str,
        view_id: &str,
        params: Option<serde_json::Value>,
        source: &str,
        now: Instant,
    ) -> Result<Vec<String>, TooManyOpen> {
        let mut inner = self.inner.lock_or_recover();
        let mut evict = Vec::new();
        if !inner.records.contains_key(label) && inner.records.len() >= MAX_WINDOWS {
            // The registry never holds more than MAX_WINDOWS, so one
            // eviction makes room. With none hidden, nothing is touched.
            let Some(oldest) = inner.least_recently_hidden() else {
                return Err(TooManyOpen);
            };
            inner.take(&oldest, now, false);
            evict.push(oldest);
        }
        let token = inner.next();
        // A dying mark for this label stays: the old window's late
        // `Destroyed` event must not remove this new record.
        inner.records.insert(
            label.to_string(),
            Record {
                extension_id: extension_id.to_string(),
                view_id: view_id.to_string(),
                params,
                open_seq: 1,
                source: source.to_string(),
                booted: false,
                visible: true,
                focused: false,
                hidden_token: token,
                hidden_seq: 0,
                last_hidden_at: None,
                geometry_token: 0,
                geometry_pending: false,
                compose_times: VecDeque::new(),
                copy_times: VecDeque::new(),
            },
        );
        Ok(evict)
    }

    /// An existing window opened again: new params and source, one more
    /// in the sequence, visible. Cancels a pending teardown. `None` when
    /// there is no record.
    pub fn reopen(
        &self,
        label: &str,
        params: Option<serde_json::Value>,
        source: &str,
    ) -> Option<Reopened> {
        let mut inner = self.inner.lock_or_recover();
        let token = inner.next();
        let record = inner.records.get_mut(label)?;
        record.open_seq += 1;
        record.params = params;
        record.source = source.to_string();
        let was_hidden = !record.visible;
        record.visible = true;
        record.hidden_token = token;
        Some(Reopened {
            open_seq: record.open_seq,
            booted: record.booted,
            was_hidden,
        })
    }

    /// The boot record of `label`, marking the page booted. `reloaded`
    /// says whether it had booted before.
    pub fn boot(&self, label: &str) -> Option<BootRecord> {
        let mut inner = self.inner.lock_or_recover();
        let record = inner.records.get_mut(label)?;
        let reloaded = record.booted;
        record.booted = true;
        Some(BootRecord {
            extension_id: record.extension_id.clone(),
            view_id: record.view_id.clone(),
            params: record.params.clone(),
            open_seq: record.open_seq,
            source: record.source.clone(),
            reloaded,
        })
    }

    /// The window of `label` was hidden. Returns the teardown token and
    /// the hidden windows to evict beyond [`MAX_HIDDEN`].
    pub fn mark_hidden(&self, label: &str, now: Instant) -> Option<Hidden> {
        let mut inner = self.inner.lock_or_recover();
        let token = inner.next();
        let seq = inner.next();
        let record = inner.records.get_mut(label)?;
        record.visible = false;
        record.focused = false;
        record.hidden_token = token;
        record.hidden_seq = seq;
        record.last_hidden_at = Some(now);
        let mut evict = Vec::new();
        while inner.hidden_count() > MAX_HIDDEN {
            let Some(oldest) = inner.least_recently_hidden() else {
                break;
            };
            inner.take(&oldest, now, false);
            evict.push(oldest);
        }
        drop(inner);
        self.focus_changed.notify_waiters();
        Some(Hidden { token, evict })
    }

    /// Whether the teardown timer with `token` should destroy `label`:
    /// the window is still hidden and wasn't shown since.
    pub fn teardown_due(&self, label: &str, token: u64) -> bool {
        self.inner
            .lock_or_recover()
            .records
            .get(label)
            .is_some_and(|r| !r.visible && r.hidden_token == token)
    }

    /// Remove the record of a window the caller is about to destroy. With
    /// `refresh`, a visible window's `Destroyed` event refreshes the
    /// activation policy. Returns whether a record existed.
    pub fn take_for_destroy(&self, label: &str, now: Instant, refresh: bool) -> bool {
        let taken = self.inner.lock_or_recover().take(label, now, refresh);
        if taken.is_some() {
            self.focus_changed.notify_waiters();
        }
        taken.is_some()
    }

    /// Drop a record whose window was never built (the build failed).
    pub fn remove(&self, label: &str) {
        self.inner.lock_or_recover().records.remove(label);
    }

    /// The window of `label` is gone. Clears its dying mark, or removes
    /// its record if nothing destroyed it on purpose (the OS did, or the
    /// mark is older than [`DYING_TIMEOUT`]). Returns whether a visible
    /// window went, so the activation policy needs a refresh.
    pub fn on_destroyed(&self, label: &str, now: Instant) -> bool {
        let mut inner = self.inner.lock_or_recover();
        let dying = inner
            .dying
            .remove(label)
            .filter(|d| now.duration_since(d.since) < DYING_TIMEOUT);
        let refresh = match dying {
            Some(dying) => dying.was_visible,
            None => inner.records.remove(label).is_some_and(|r| r.visible),
        };
        drop(inner);
        self.focus_changed.notify_waiters();
        refresh
    }

    /// Labels of every window of `extension_id`.
    pub fn labels_for_extension(&self, extension_id: &str) -> Vec<String> {
        let mut labels: Vec<String> = self
            .inner
            .lock_or_recover()
            .records
            .iter()
            .filter(|(_, r)| r.extension_id == extension_id)
            .map(|(label, _)| label.clone())
            .collect();
        labels.sort();
        labels
    }

    /// Whether the window of `label` is visible (false without a record).
    #[cfg(test)]
    pub fn is_visible(&self, label: &str) -> bool {
        self.inner
            .lock_or_recover()
            .records
            .get(label)
            .is_some_and(|r| r.visible)
    }

    /// A view window gained or lost focus.
    pub fn set_focused(&self, label: &str, focused: bool) {
        if let Some(record) = self.inner.lock_or_recover().records.get_mut(label) {
            record.focused = focused;
        }
        self.focus_changed.notify_waiters();
    }

    /// Whether the window of `label` has focus, as its events said.
    pub fn is_focused(&self, label: &str) -> bool {
        self.inner
            .lock_or_recover()
            .records
            .get(label)
            .is_some_and(|r| r.focused)
    }

    /// Labels of the view windows that have focus.
    pub fn focused_labels(&self) -> Vec<String> {
        self.inner
            .lock_or_recover()
            .records
            .iter()
            .filter(|(_, r)| r.focused)
            .map(|(label, _)| label.clone())
            .collect()
    }

    /// For the launcher's blur handler: `false` at once when no view
    /// window is visible; otherwise whether one has focus or gains it
    /// within `within` (the launcher's blur and the view's focus can
    /// arrive in either order).
    pub async fn wait_for_view_focus(&self, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let notified = self.focus_changed.notified();
            tokio::pin!(notified);
            // Register before reading the state, so a change in between
            // still wakes us.
            notified.as_mut().enable();
            {
                let inner = self.inner.lock_or_recover();
                if !inner.records.values().any(|r| r.visible) {
                    return false;
                }
                if inner.records.values().any(|r| r.focused) {
                    return true;
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return false;
            }
        }
    }

    /// Every window, sorted by label, for Settings.
    pub fn list(&self) -> Vec<ExtViewInfo> {
        let mut out: Vec<(String, ExtViewInfo)> = self
            .inner
            .lock_or_recover()
            .records
            .iter()
            .map(|(label, r)| {
                (
                    label.clone(),
                    ExtViewInfo {
                        extension_id: r.extension_id.clone(),
                        view_id: r.view_id.clone(),
                        visible: r.visible,
                        focused: r.focused,
                    },
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.into_iter().map(|(_, info)| info).collect()
    }

    /// A move or resize: a new debounce token, and whether the caller
    /// should start the debounce task (none is running). `None` without a
    /// record.
    pub fn bump_geometry(&self, label: &str) -> Option<(u64, bool)> {
        let mut inner = self.inner.lock_or_recover();
        let token = inner.next();
        let record = inner.records.get_mut(label)?;
        record.geometry_token = token;
        let start = !record.geometry_pending;
        record.geometry_pending = true;
        Some((token, start))
    }

    /// After the debounce wait for `token`: wait again, save, or skip.
    /// Clears the pending flag unless it waits again.
    pub fn settle_geometry(&self, label: &str, token: u64) -> Settle {
        let mut inner = self.inner.lock_or_recover();
        let Some(record) = inner.records.get_mut(label) else {
            return Settle::Skip;
        };
        if record.geometry_token != token {
            return Settle::Changed(record.geometry_token);
        }
        record.geometry_pending = false;
        if record.visible {
            Settle::Save
        } else {
            Settle::Skip
        }
    }

    /// Count a compose or copy request; `false` when it is over
    /// [`RATE_MAX`] in the last [`RATE_WINDOW`] (or there's no record).
    pub fn allow(&self, label: &str, kind: RateKind, now: Instant) -> bool {
        let mut inner = self.inner.lock_or_recover();
        let Some(record) = inner.records.get_mut(label) else {
            return false;
        };
        let times = match kind {
            RateKind::Compose => &mut record.compose_times,
            RateKind::Copy => &mut record.copy_times,
        };
        while times
            .front()
            .is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW)
        {
            times.pop_front();
        }
        if times.len() >= RATE_MAX {
            return false;
        }
        times.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn label(view: &str) -> String {
        format!("ext-view:mindmap:{view}")
    }

    fn open(reg: &Registry, view: &str, now: Instant) -> Result<Vec<String>, TooManyOpen> {
        reg.reserve(&label(view), "mindmap", view, None, "launcher", now)
    }

    #[test]
    fn open_boot_reopen() {
        let reg = Registry::default();
        let now = Instant::now();
        let l = label("explorer");
        assert_eq!(
            reg.reserve(
                &l,
                "mindmap",
                "explorer",
                Some(json!({"a": 1})),
                "chat",
                now
            ),
            Ok(vec![])
        );
        assert!(reg.contains(&l));
        assert!(reg.is_visible(&l));
        let boot = reg.boot(&l).unwrap();
        assert_eq!(boot.extension_id, "mindmap");
        assert_eq!(boot.view_id, "explorer");
        assert_eq!(boot.params, Some(json!({"a": 1})));
        assert_eq!(boot.open_seq, 1);
        assert_eq!(boot.source, "chat");
        assert!(!boot.reloaded);

        let again = reg.reopen(&l, Some(json!({"b": 2})), "widget").unwrap();
        assert_eq!(
            again,
            Reopened {
                open_seq: 2,
                booted: true,
                was_hidden: false
            }
        );
        // A second boot is a reload in place, and sees the newest params.
        let boot = reg.boot(&l).unwrap();
        assert!(boot.reloaded);
        assert_eq!(boot.open_seq, 2);
        assert_eq!(boot.params, Some(json!({"b": 2})));
        assert_eq!(boot.source, "widget");
        assert_eq!(reg.reopen("ext-view:other:x", None, "launcher"), None);
        assert_eq!(reg.boot("ext-view:other:x"), None);
    }

    #[test]
    fn reopen_before_boot_keeps_the_newest_params() {
        let reg = Registry::default();
        let l = label("explorer");
        open(&reg, "explorer", Instant::now()).unwrap();
        let again = reg.reopen(&l, Some(json!({"focus": "x"})), "launcher");
        assert_eq!(again.map(|r| r.booted), Some(false));
        let again = reg.reopen(&l, Some(json!({"focus": "y"})), "launcher");
        assert_eq!(again.map(|r| r.open_seq), Some(3));
        let boot = reg.boot(&l).unwrap();
        assert_eq!(boot.params, Some(json!({"focus": "y"})));
        assert_eq!(boot.open_seq, 3);
        assert!(!boot.reloaded);
    }

    #[test]
    fn teardown_token_is_invalidated_by_a_reopen() {
        let reg = Registry::default();
        let now = Instant::now();
        let l = label("explorer");
        open(&reg, "explorer", now).unwrap();
        let hidden = reg.mark_hidden(&l, now).unwrap();
        assert!(hidden.evict.is_empty());
        assert!(reg.teardown_due(&l, hidden.token));
        let again = reg.reopen(&l, None, "launcher").unwrap();
        assert!(again.was_hidden);
        assert!(!reg.teardown_due(&l, hidden.token));
        // Hidden again: the old timer still can't fire, the new one can.
        let second = reg.mark_hidden(&l, now).unwrap();
        assert!(!reg.teardown_due(&l, hidden.token));
        assert!(reg.teardown_due(&l, second.token));
        assert!(!reg.teardown_due("ext-view:nope:x", second.token));
    }

    #[test]
    fn a_fifth_window_evicts_the_least_recently_hidden() {
        let reg = Registry::default();
        let now = Instant::now();
        for v in ["a", "b", "c", "d"] {
            assert_eq!(open(&reg, v, now), Ok(vec![]));
        }
        // b hidden first, then c: b is the least recently hidden.
        reg.mark_hidden(&label("b"), now).unwrap();
        reg.mark_hidden(&label("c"), now).unwrap();
        assert_eq!(open(&reg, "e", now), Ok(vec![label("b")]));
        assert!(!reg.contains(&label("b")));
        assert!(reg.is_dying(&label("b"), now));
        assert_eq!(reg.list().len(), 4);
    }

    #[test]
    fn all_visible_refuses_a_fifth() {
        let reg = Registry::default();
        let now = Instant::now();
        for v in ["a", "b", "c", "d"] {
            open(&reg, v, now).unwrap();
        }
        assert_eq!(open(&reg, "e", now), Err(TooManyOpen));
        assert_eq!(reg.list().len(), 4);
        assert!(!reg.contains(&label("e")));
        // Opening one that exists isn't a new window.
        assert_eq!(open(&reg, "a", now), Ok(vec![]));
    }

    #[test]
    fn a_third_hidden_window_evicts_the_least_recently_hidden() {
        let reg = Registry::default();
        let now = Instant::now();
        for v in ["a", "b", "c"] {
            open(&reg, v, now).unwrap();
        }
        assert_eq!(
            reg.mark_hidden(&label("a"), now).unwrap().evict,
            vec![] as Vec<String>
        );
        assert_eq!(
            reg.mark_hidden(&label("b"), now).unwrap().evict,
            vec![] as Vec<String>
        );
        let hidden = reg.mark_hidden(&label("c"), now).unwrap();
        assert_eq!(hidden.evict, vec![label("a")]);
        assert!(!reg.contains(&label("a")));
        assert!(reg.contains(&label("b")));
        assert!(reg.contains(&label("c")));
    }

    #[test]
    fn destroy_marks_dying_until_the_event() {
        let reg = Registry::default();
        let now = Instant::now();
        let l = label("explorer");
        open(&reg, "explorer", now).unwrap();
        assert!(reg.take_for_destroy(&l, now, false));
        assert!(!reg.contains(&l));
        assert!(reg.is_dying(&l, now));
        assert!(!reg.is_dying(&l, now + DYING_TIMEOUT));
        // A new window with the same label isn't removed by the old one's
        // late Destroyed event.
        open(&reg, "explorer", now).unwrap();
        assert!(!reg.on_destroyed(&l, now));
        assert!(reg.contains(&l));
        assert!(!reg.is_dying(&l, now));
        // An OS-initiated destroy removes the record (a visible one asks
        // for a policy refresh).
        assert!(reg.on_destroyed(&l, now));
        assert!(!reg.contains(&l));
        assert!(!reg.take_for_destroy(&l, now, true));
    }

    #[test]
    fn a_stale_dying_mark_does_not_shield_a_record() {
        let reg = Registry::default();
        let now = Instant::now();
        let l = label("explorer");
        open(&reg, "explorer", now).unwrap();
        reg.take_for_destroy(&l, now, false);
        open(&reg, "explorer", now).unwrap();
        // The old window's event never came; the new one goes much later.
        assert!(reg.on_destroyed(&l, now + DYING_TIMEOUT));
        assert!(!reg.contains(&l));
    }

    #[test]
    fn a_visible_window_destroyed_on_purpose_refreshes_the_policy_once_gone() {
        let reg = Registry::default();
        let now = Instant::now();
        let l = label("explorer");
        open(&reg, "explorer", now).unwrap();
        reg.take_for_destroy(&l, now, true);
        assert!(reg.on_destroyed(&l, now));
        open(&reg, "explorer", now).unwrap();
        reg.take_for_destroy(&l, now, false);
        assert!(!reg.on_destroyed(&l, now));
        open(&reg, "explorer", now).unwrap();
        reg.mark_hidden(&l, now);
        reg.take_for_destroy(&l, now, true);
        assert!(!reg.on_destroyed(&l, now), "a hidden window isn't counted");
    }

    #[test]
    fn labels_by_extension_and_list() {
        let reg = Registry::default();
        let now = Instant::now();
        reg.reserve("ext-view:b:x", "b", "x", None, "launcher", now)
            .unwrap();
        reg.reserve("ext-view:a:y", "a", "y", None, "launcher", now)
            .unwrap();
        reg.reserve("ext-view:a:x", "a", "x", None, "launcher", now)
            .unwrap();
        assert_eq!(
            reg.labels_for_extension("a"),
            vec!["ext-view:a:x".to_string(), "ext-view:a:y".to_string()]
        );
        assert!(reg.labels_for_extension("c").is_empty());
        reg.mark_hidden("ext-view:a:y", now);
        reg.set_focused("ext-view:b:x", true);
        let list = reg.list();
        assert_eq!(
            list.iter()
                .map(|i| (
                    i.extension_id.as_str(),
                    i.view_id.as_str(),
                    i.visible,
                    i.focused
                ))
                .collect::<Vec<_>>(),
            vec![
                ("a", "x", true, false),
                ("a", "y", false, false),
                ("b", "x", true, true)
            ]
        );
    }

    #[test]
    fn focus_flags_follow_events_and_hiding_clears_them() {
        let reg = Registry::default();
        let now = Instant::now();
        let l = label("explorer");
        open(&reg, "explorer", now).unwrap();
        reg.set_focused(&l, true);
        assert!(reg.is_focused(&l));
        assert_eq!(reg.focused_labels(), vec![l.clone()]);
        reg.mark_hidden(&l, now);
        assert!(!reg.is_focused(&l));
        assert!(reg.focused_labels().is_empty());
        // An event for a window without a record is ignored.
        reg.set_focused("ext-view:nope:x", true);
        assert!(!reg.is_focused("ext-view:nope:x"));
    }

    #[tokio::test]
    async fn focus_wait_answers_at_once_without_a_visible_view() {
        let reg = Registry::default();
        let started = std::time::Instant::now();
        assert!(!reg.wait_for_view_focus(Duration::from_secs(5)).await);
        let now = Instant::now();
        open(&reg, "explorer", now).unwrap();
        reg.mark_hidden(&label("explorer"), now);
        assert!(!reg.wait_for_view_focus(Duration::from_secs(5)).await);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn focus_wait_sees_focus_that_arrives_late() {
        let reg = std::sync::Arc::new(Registry::default());
        open(&reg, "explorer", Instant::now()).unwrap();
        // No focus within the wait: false after about the wait.
        let started = std::time::Instant::now();
        assert!(!reg.wait_for_view_focus(Duration::from_millis(60)).await);
        assert!(started.elapsed() >= Duration::from_millis(50));

        let late = reg.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            late.set_focused(&label("explorer"), true);
        });
        assert!(reg.wait_for_view_focus(Duration::from_secs(5)).await);
        // Already focused: true at once.
        assert!(reg.wait_for_view_focus(Duration::from_millis(1)).await);
    }

    #[test]
    fn geometry_debounce() {
        let reg = Registry::default();
        let now = Instant::now();
        let l = label("explorer");
        assert_eq!(reg.bump_geometry(&l), None);
        open(&reg, "explorer", now).unwrap();
        let (first, start) = reg.bump_geometry(&l).unwrap();
        assert!(start);
        let (second, start) = reg.bump_geometry(&l).unwrap();
        assert!(!start, "one task per window");
        assert_eq!(reg.settle_geometry(&l, first), Settle::Changed(second));
        assert_eq!(reg.settle_geometry(&l, second), Settle::Save);
        let (third, start) = reg.bump_geometry(&l).unwrap();
        assert!(start, "the settled task is done");
        reg.mark_hidden(&l, now);
        assert_eq!(reg.settle_geometry(&l, third), Settle::Skip);
        assert_eq!(reg.settle_geometry("ext-view:nope:x", third), Settle::Skip);
    }

    #[test]
    fn rate_limit_is_two_per_second_per_kind() {
        let reg = Registry::default();
        let now = Instant::now();
        let l = label("explorer");
        assert!(!reg.allow(&l, RateKind::Copy, now), "no record");
        open(&reg, "explorer", now).unwrap();
        assert!(reg.allow(&l, RateKind::Copy, now));
        assert!(reg.allow(&l, RateKind::Copy, now + Duration::from_millis(100)));
        assert!(!reg.allow(&l, RateKind::Copy, now + Duration::from_millis(200)));
        // Compose has its own budget.
        assert!(reg.allow(&l, RateKind::Compose, now + Duration::from_millis(200)));
        // A second after the first, one slot is free again.
        assert!(reg.allow(&l, RateKind::Copy, now + Duration::from_millis(1000)));
        assert!(!reg.allow(&l, RateKind::Copy, now + Duration::from_millis(1050)));
        assert!(reg.allow(&l, RateKind::Copy, now + Duration::from_millis(1100)));
    }
}

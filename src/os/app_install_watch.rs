//! Notice app installs and uninstalls as they happen, so the launcher can
//! rescan right away instead of on its hourly timer.
//!
//! Neither OS has a single "an app was installed" event, so this listens
//! to the places installers write:
//!
//! - **Every platform:** filesystem events (FSEvents / ReadDirectoryChangesW
//!   / inotify via `notify`) on the directories the launcher scans, from
//!   `kage_core::os::launcher::watch_roots` — `/Applications` and friends on
//!   macOS (recursively, for vendor folders), the two Start Menu `Programs`
//!   trees on Windows, the XDG `applications/` dirs on Linux. A root that
//!   doesn't exist yet (no `~/Applications` until the first Chrome web app;
//!   no flatpak dir until the first `flatpak install --user`) is polled for
//!   by the caller (`AppInstallWatch::watch_new_roots`) and watched once it
//!   appears. Polling, not a stand-in watch on the parent: on macOS that
//!   parent is the home folder, and an FSEvents stream there wakes us for
//!   every cache write under ~/Library.
//! - **Windows extras** (`os::windows::app_install_watch`): registry change
//!   notifications on the `Uninstall` key the scanner reads, and the
//!   WinRT `PackageCatalog` install/uninstall events for Store/MSIX apps,
//!   which create no Start Menu shortcut.
//!
//! Which filesystem changes count is per platform
//! (`is_relevant_change_impl`); on macOS only bundle- and folder-level
//! changes do, so a log a local server writes into its vendor folder
//! doesn't rescan.
//!
//! Signals are coarse — "something changed, rescan" — and arrive in bursts
//! while an installer runs. Debouncing is the caller's job
//! (`setup::spawn_app_registry_scan`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::lock_ext::LockExt;
use kage_core::os::launcher::WatchRoot;
use notify::{Event, EventKind, RecursiveMode, Watcher};

/// Invoked (from a watcher thread) whenever the installed-app set may have
/// changed. Must be cheap and non-blocking.
pub type ChangeCallback = Arc<dyn Fn() + Send + Sync + 'static>;

/// Which roots are watched (shared with the callback thread, which needs
/// them to classify events) and which don't exist yet.
#[derive(Debug, Default)]
struct WatchState {
    watched: Vec<PathBuf>,
    pending: Vec<WatchRoot>,
}

/// Keeps the watchers alive. Dropping it stops the filesystem watcher;
/// the Windows registry / package-catalog listeners live for the rest of
/// the process.
pub struct AppInstallWatch {
    fs: Option<notify::RecommendedWatcher>,
    state: Arc<Mutex<WatchState>>,
    on_change: ChangeCallback,
}

/// Start every available listener. Failures are logged and leave that
/// source unwatched — the periodic rescan still covers it.
pub fn start(on_change: ChangeCallback) -> AppInstallWatch {
    let watch = AppInstallWatch::watch(&kage_core::os::launcher::watch_roots(), on_change.clone());
    crate::os::platform::app_install_watch::start_extra_impl(on_change);
    watch
}

fn mode(root: &WatchRoot) -> RecursiveMode {
    if root.recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    }
}

impl AppInstallWatch {
    /// Watch `roots` for changes that could add or remove an app.
    pub fn watch(roots: &[WatchRoot], on_change: ChangeCallback) -> Self {
        let state = Arc::new(Mutex::new(WatchState::default()));
        let cb_state = state.clone();
        let cb = on_change.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
            Ok(event) => {
                let relevant = is_relevant(&cb_state.lock_or_recover(), &event);
                if relevant {
                    log::debug!("App folder change: {:?} {:?}", event.kind, event.paths);
                    cb();
                }
            }
            // Lost events (queue overflow etc.) — rescanning is the safe reply.
            Err(e) => {
                log::debug!("App folder watcher error: {}", e);
                cb();
            }
        });
        let mut fs = match watcher {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("Could not create app folder watcher: {}", e);
                None
            }
        };

        // Never hold `state` across a `watcher` call: notify's FSEvents
        // backend restarts its stream on every watch()/unwatch() and spins
        // until the stream thread is idle — which it never is if that
        // thread is blocked on `state` inside our callback.
        if let Some(watcher) = fs.as_mut() {
            for root in roots {
                if !root.path.is_dir() {
                    state.lock_or_recover().pending.push(root.clone());
                    continue;
                }
                match watcher.watch(&root.path, mode(root)) {
                    Ok(()) => state.lock_or_recover().watched.push(root.path.clone()),
                    Err(e) => log::warn!(
                        "Could not watch {} for app installs: {}",
                        root.path.display(),
                        e
                    ),
                }
            }
            let st = state.lock_or_recover();
            log::info!(
                "Watching {} app folder(s) for installs ({} not created yet)",
                st.watched.len(),
                st.pending.len()
            );
        }
        Self {
            fs,
            state,
            on_change,
        }
    }

    /// Whether some root didn't exist yet; the caller should then call
    /// `watch_new_roots` every so often.
    pub fn has_pending_roots(&self) -> bool {
        self.fs.is_some() && !self.state.lock_or_recover().pending.is_empty()
    }

    /// Start watching any root that has appeared since the last call. One
    /// that turns up triggers a rescan, since apps may have landed in it
    /// before its watch existed. Cheap (a `stat` per missing root).
    pub fn watch_new_roots(&mut self) {
        let Some(watcher) = self.fs.as_mut() else {
            return;
        };
        // Lock released before calling the watcher; see `watch`.
        let appeared: Vec<WatchRoot> = self
            .state
            .lock_or_recover()
            .pending
            .iter()
            .filter(|r| r.path.is_dir())
            .cloned()
            .collect();
        let mut picked_up = false;
        for root in appeared {
            let result = watcher.watch(&root.path, mode(&root));
            let mut st = self.state.lock_or_recover();
            // Either way, stop polling for it: a folder we can't watch
            // still gets the hourly rescan, without a warning every poll.
            st.pending.retain(|r| r.path != root.path);
            match result {
                Ok(()) => {
                    log::info!("Now watching {} for app installs", root.path.display());
                    st.watched.push(root.path);
                }
                Err(e) => {
                    log::warn!(
                        "Could not watch {} for app installs: {}",
                        root.path.display(),
                        e
                    )
                }
            }
            // It exists now either way, and may already hold apps.
            picked_up = true;
        }
        if picked_up {
            (self.on_change)();
        }
    }
}

/// Whether `event` could change the scanned app list: the platform rule
/// (`is_relevant_change`) applied to each path, relative to the watched
/// root it falls under.
fn is_relevant(st: &WatchState, event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    if event.need_rescan() || event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|p| {
        st.watched
            .iter()
            .find(|root| p.starts_with(root))
            .is_some_and(|root| is_relevant_change(&event.kind, root, p))
    })
}

/// Whether a change of `kind` at `path`, under watched root `root`, could
/// add or remove an app. Reads never can, and neither can hidden
/// housekeeping files (Finder's `.DS_Store`, `.localized`, Explorer's
/// `desktop.ini`) — Finder rewrites `.DS_Store` just from browsing
/// `/Applications`. The rest is platform policy.
pub fn is_relevant_change(kind: &EventKind, root: &Path, path: &Path) -> bool {
    !matches!(kind, EventKind::Access(_))
        && !is_housekeeping_file(path)
        && crate::os::platform::app_install_watch::is_relevant_change_impl(kind, root, path)
}

fn is_housekeeping_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.starts_with('.') || name.eq_ignore_ascii_case("desktop.ini")
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, MetadataKind, ModifyKind, RemoveKind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// Classify `path` under the root that is its first two components
    /// (`/Applications`, `/usr/share`, `Start Menu/Programs`).
    fn change(kind: EventKind, path: &str) -> bool {
        let path = Path::new(path);
        let root: PathBuf = path.components().take(2).collect();
        is_relevant_change(&kind, &root, path)
    }

    fn state(watched: &[&str]) -> WatchState {
        WatchState {
            watched: watched.iter().map(PathBuf::from).collect(),
            pending: Vec::new(),
        }
    }

    #[test]
    fn reads_and_housekeeping_files_never_count() {
        assert!(!change(
            EventKind::Access(AccessKind::Any),
            "/usr/share/applications/foo.desktop"
        ));
        assert!(!change(
            EventKind::Create(CreateKind::File),
            "/Applications/.DS_Store"
        ));
        assert!(!change(
            EventKind::Modify(ModifyKind::Any),
            "Start Menu/Programs/Desktop.ini"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_counts_bundle_and_folder_level_changes_only() {
        use notify::event::{DataChange, RenameMode};
        let create_dir = EventKind::Create(CreateKind::Folder);
        let rename = EventKind::Modify(ModifyKind::Name(RenameMode::Any));
        // Installs, removals and moves of apps — including Finder's
        // end-of-copy metadata touch on the bundle.
        assert!(change(create_dir, "/Applications/Foo.app"));
        assert!(change(
            rename,
            "/Applications/Cisco/Cisco Secure Client.app"
        ));
        assert!(change(
            EventKind::Remove(RemoveKind::Any),
            "/Applications/Foo.app"
        ));
        assert!(change(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Ownership)),
            "/Applications/Foo.app"
        ));
        // Vendor folders appearing / disappearing.
        assert!(change(create_dir, "/Applications/Cisco"));
        assert!(change(
            EventKind::Remove(RemoveKind::Folder),
            "/Applications/Cisco"
        ));
        assert!(change(rename, "/definitely/missing/kage/Vendor"));
        // Data a local server writes into its vendor folder (MAMP/XAMPP).
        assert!(!change(
            EventKind::Create(CreateKind::File),
            "/Applications/MAMP/logs/apache_access.log"
        ));
        assert!(!change(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            "/Applications/MAMP/logs/apache_access.log"
        ));
        assert!(!change(
            rename,
            "/definitely/missing/kage/MAMP/logs/apache_access.log.1"
        ));
        // Writes inside a bundle, and updater staging in hidden folders.
        assert!(!change(
            create_dir,
            "/Applications/Google Chrome.app/Contents/Frameworks/New.framework"
        ));
        assert!(!change(create_dir, "/Applications/Vendor/.tmp/Foo.app"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn shortcut_and_desktop_entry_changes_count_but_metadata_does_not() {
        assert!(change(
            EventKind::Remove(RemoveKind::Any),
            "Start Menu/Programs/Foo/Foo.lnk"
        ));
        assert!(change(
            EventKind::Modify(ModifyKind::Any),
            "/usr/share/applications/foo.desktop"
        ));
        assert!(!change(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            "/usr/share/applications/foo.desktop"
        ));
    }

    #[test]
    fn rescan_requests_and_pathless_events_are_relevant() {
        let st = state(&["/Applications"]);
        assert!(is_relevant(&st, &Event::new(EventKind::Any)));
        assert!(is_relevant(
            &st,
            &Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)
        ));
        assert!(!is_relevant(
            &st,
            &Event::new(EventKind::Access(AccessKind::Any))
        ));
    }

    #[test]
    fn paths_outside_every_watched_root_are_ignored() {
        let st = state(&["/Applications"]);
        let ev = Event::new(EventKind::Create(CreateKind::Folder))
            .add_path(PathBuf::from("/Users/me/Applications/Foo.app"));
        assert!(!is_relevant(&st, &ev));
    }

    fn counter() -> (Arc<AtomicUsize>, ChangeCallback) {
        let hits = Arc::new(AtomicUsize::new(0));
        let c = hits.clone();
        (
            hits,
            Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    fn wait_for(hits: &AtomicUsize, at_least: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while hits.load(Ordering::SeqCst) < at_least && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        hits.load(Ordering::SeqCst) >= at_least
    }

    /// End to end against the real OS watcher: creating an app bundle in a
    /// watched folder fires the callback.
    #[test]
    fn creating_an_app_in_a_watched_folder_fires() {
        let dir = tempfile::tempdir().unwrap();
        // FSEvents reports resolved paths (/private/var/… for a /var tempdir).
        let root = dir.path().canonicalize().unwrap();
        let (hits, cb) = counter();
        let _watch = AppInstallWatch::watch(
            &[WatchRoot {
                path: root.clone(),
                recursive: true,
            }],
            cb,
        );
        // FSEvents can drop events for changes made right after the stream
        // starts; give it a moment to come up.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::create_dir(root.join("Foo.app")).unwrap();
        assert!(wait_for(&hits, 1), "no change notification within 10 s");
    }

    /// A root that doesn't exist at startup (first Chrome web app creates
    /// ~/Applications) is picked up when it appears.
    #[test]
    fn a_root_created_later_is_noticed_and_then_watched() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let root = base.join("Applications");
        let (hits, cb) = counter();
        let mut watch = AppInstallWatch::watch(
            &[WatchRoot {
                path: root.clone(),
                recursive: true,
            }],
            cb,
        );
        assert!(watch.has_pending_roots());
        watch.watch_new_roots();
        assert!(watch.has_pending_roots(), "still missing");
        assert_eq!(hits.load(Ordering::SeqCst), 0);

        // The caller's poll picks the root up and asks for a rescan.
        std::fs::create_dir(&root).unwrap();
        watch.watch_new_roots();
        assert!(!watch.has_pending_roots());
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        std::thread::sleep(Duration::from_millis(300));
        let before = hits.load(Ordering::SeqCst);
        std::fs::create_dir_all(root.join("Chrome Apps.localized/Gmail.app")).unwrap();
        assert!(
            wait_for(&hits, before + 1),
            "app in the new root not noticed"
        );
    }
}

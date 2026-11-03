//! Notice app installs and uninstalls as they happen, so the launcher can
//! rescan right away instead of on its hourly timer.
//!
//! Neither OS has a single "an app was installed" event, so this listens
//! to the places installers write:
//!
//! - **Every platform:** filesystem events (FSEvents / ReadDirectoryChangesW
//!   / inotify via `notify`) on the directories the launcher scans, from
//!   `kage_core::os::launcher::watch_roots` — `/Applications` and friends on
//!   macOS, the two Start Menu `Programs` trees on Windows, the XDG
//!   `applications/` dirs on Linux.
//! - **Windows extras** (`os::windows::app_install_watch`): registry change
//!   notifications on the `Uninstall` key the scanner reads, and the
//!   WinRT `PackageCatalog` install/uninstall events for Store/MSIX apps,
//!   which create no Start Menu shortcut.
//!
//! Signals are coarse — "something changed, rescan" — and arrive in bursts
//! while an installer runs. Debouncing is the caller's job
//! (`setup::spawn_app_registry_scan`).

use std::path::Path;
use std::sync::Arc;

use kage_core::os::launcher::WatchRoot;
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecursiveMode, Watcher};

/// Invoked (from a watcher thread) whenever the installed-app set may have
/// changed. Must be cheap and non-blocking.
pub type ChangeCallback = Arc<dyn Fn() + Send + Sync + 'static>;

/// Keeps the watchers alive. Dropping it stops the filesystem watcher;
/// the Windows registry / package-catalog listeners live for the rest of
/// the process.
pub struct AppInstallWatch {
    _fs: Option<notify::RecommendedWatcher>,
}

/// Start every available listener. Failures are logged and leave that
/// source unwatched — the periodic rescan still covers it.
pub fn start(on_change: ChangeCallback) -> AppInstallWatch {
    let fs = watch_roots(&kage_core::os::launcher::watch_roots(), on_change.clone());
    crate::os::platform::app_install_watch::start_extra_impl(on_change);
    AppInstallWatch { _fs: fs }
}

/// Watch `roots` for changes that could add or remove an app. Missing
/// roots are skipped. Returns `None` if nothing could be watched.
pub fn watch_roots(
    roots: &[WatchRoot],
    on_change: ChangeCallback,
) -> Option<notify::RecommendedWatcher> {
    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| match res
    {
        Ok(event) if is_relevant(&event) => {
            log::debug!("App folder change: {:?} {:?}", event.kind, event.paths);
            on_change();
        }
        Ok(_) => {}
        // Lost events (queue overflow etc.) — rescanning is the safe reply.
        Err(e) => {
            log::debug!("App folder watcher error: {}", e);
            on_change();
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("Could not create app folder watcher: {}", e);
            return None;
        }
    };

    let mut watched = 0;
    for root in roots {
        if !root.path.is_dir() {
            continue;
        }
        let mode = if root.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        match watcher.watch(&root.path, mode) {
            Ok(()) => watched += 1,
            Err(e) => log::warn!(
                "Could not watch {} for app installs: {}",
                root.path.display(),
                e
            ),
        }
    }
    log::info!("Watching {} app folder(s) for installs", watched);
    (watched > 0).then_some(watcher)
}

/// Whether `event` could change the scanned app list. Reads and
/// permission/xattr/timestamp changes can't, and neither can hidden
/// housekeeping files (Finder's `.DS_Store`, `.localized`, Explorer's
/// `desktop.ini`) — Finder rewrites `.DS_Store` just from browsing
/// `/Applications`.
pub fn is_relevant(event: &Event) -> bool {
    if matches!(
        event.kind,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_))
    ) {
        return false;
    }
    if event.need_rescan() || event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|p| !is_housekeeping_file(p))
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
    use notify::event::{AccessKind, CreateKind, MetadataKind, RemoveKind, RenameMode};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn event(kind: EventKind, path: &str) -> Event {
        Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn installs_renames_and_removals_are_relevant() {
        assert!(is_relevant(&event(
            EventKind::Create(CreateKind::Folder),
            "/Applications/Foo.app"
        )));
        assert!(is_relevant(&event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            "/Applications/Foo.app"
        )));
        assert!(is_relevant(&event(
            EventKind::Remove(RemoveKind::Any),
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Foo\Foo.lnk"
        )));
        assert!(is_relevant(&event(
            EventKind::Modify(ModifyKind::Any),
            "/usr/share/applications/foo.desktop"
        )));
    }

    #[test]
    fn reads_metadata_and_housekeeping_files_are_ignored() {
        assert!(!is_relevant(&event(
            EventKind::Access(AccessKind::Any),
            "/usr/share/applications/foo.desktop"
        )));
        assert!(!is_relevant(&event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Extended)),
            "/Applications/Foo.app"
        )));
        assert!(!is_relevant(&event(
            EventKind::Create(CreateKind::File),
            "/Applications/.DS_Store"
        )));
        assert!(!is_relevant(&event(
            EventKind::Modify(ModifyKind::Any),
            "Start Menu/Programs/Desktop.ini"
        )));
    }

    #[test]
    fn rescan_requests_and_pathless_events_are_relevant() {
        assert!(is_relevant(&Event::new(EventKind::Any)));
        assert!(is_relevant(
            &Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)
        ));
    }

    #[test]
    fn missing_roots_are_skipped() {
        let roots = [WatchRoot {
            path: PathBuf::from("/definitely/not/a/real/kage/dir"),
            recursive: false,
        }];
        assert!(watch_roots(&roots, Arc::new(|| {})).is_none());
    }

    /// End to end against the real OS watcher: creating an app bundle /
    /// shortcut in a watched folder fires the callback.
    #[test]
    fn creating_an_app_in_a_watched_folder_fires() {
        let dir = tempfile::tempdir().unwrap();
        // FSEvents reports resolved paths (/private/var/… for a /var tempdir).
        let root = dir.path().canonicalize().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let _watcher = watch_roots(
            &[WatchRoot {
                path: root.clone(),
                recursive: false,
            }],
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .expect("temp dir is watchable");

        // FSEvents can drop events for changes made right after the stream
        // starts; give it a moment to come up.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::create_dir(root.join("Foo.app")).unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while hits.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            hits.load(Ordering::SeqCst) > 0,
            "no change notification within 10 s"
        );
    }
}

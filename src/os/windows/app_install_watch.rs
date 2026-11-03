//! Windows install signals the Start Menu filesystem watcher can't see:
//!
//! - The HKLM `Uninstall` key, which `kage_core`'s scanner reads for
//!   desktop apps that ship no Start Menu shortcut. `RegNotifyChangeKeyValue`
//!   fires when an installer adds or removes its subkey.
//! - Store / MSIX packages, which never touch the Start Menu folders.
//!   The WinRT `PackageCatalog` raises install, uninstall, update and
//!   status-change events for the current user's packages.
//!
//! Each listener runs on its own thread for the life of the process: the
//! registry wait is a blocking call, and `PackageCatalog` isn't `Send`, so
//! it stays on the thread that registered its handlers.

use crate::os::app_install_watch::ChangeCallback;
use windows::core::{w, Ref, PCWSTR};
use windows::ApplicationModel::{
    PackageCatalog, PackageInstallingEventArgs, PackageStatusChangedEventArgs,
    PackageUninstallingEventArgs, PackageUpdatingEventArgs,
};
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Registry::{
    RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_NOTIFY,
    REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME,
};

/// Same key `kage_core::os::windows::launcher::scan_registry_apps` reads.
const UNINSTALL_KEY: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");

/// The Start Menu trees hold shortcuts and folders only, so any
/// create/remove/rename/content change can matter; attribute and
/// timestamp changes can't.
pub fn is_relevant_change_impl(
    kind: &notify::EventKind,
    _root: &std::path::Path,
    _path: &std::path::Path,
) -> bool {
    !matches!(
        kind,
        notify::EventKind::Modify(notify::event::ModifyKind::Metadata(_))
    )
}

pub fn start_extra_impl(on_change: ChangeCallback) {
    let registry_cb = on_change.clone();
    spawn("app-watch-registry", move || {
        watch_uninstall_key(registry_cb)
    });
    spawn("app-watch-packages", move || {
        watch_package_catalog(on_change)
    });
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.to_string()).spawn(f) {
        log::warn!("Could not start {} thread: {}", name, e);
    }
}

fn watch_uninstall_key(on_change: ChangeCallback) {
    let mut key = HKEY::default();
    // Safety: `key` is a valid out-pointer; UNINSTALL_KEY is a static
    // NUL-terminated wide string.
    let err = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            UNINSTALL_KEY,
            None,
            KEY_NOTIFY,
            &mut key,
        )
    };
    if err != ERROR_SUCCESS {
        log::warn!(
            "Could not open the Uninstall key for change notifications: {:?}",
            err
        );
        return;
    }
    log::info!("Watching the Uninstall registry key for app installs");
    loop {
        // Synchronous mode: blocks this thread until a subkey is added or
        // removed, or a value under the key changes. One notification per
        // call, so re-arm after each.
        // Safety: `key` is an open handle with KEY_NOTIFY access.
        let err = unsafe {
            RegNotifyChangeKeyValue(
                key,
                true,
                REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET,
                None,
                false,
            )
        };
        if err != ERROR_SUCCESS {
            log::warn!("Uninstall key change notification failed: {:?}", err);
            break;
        }
        log::debug!("Uninstall registry key changed");
        on_change();
    }
    // Safety: `key` came from RegOpenKeyExW and isn't used afterwards.
    unsafe {
        let _ = RegCloseKey(key);
    }
}

fn watch_package_catalog(on_change: ChangeCallback) {
    // WinRT needs an apartment; S_FALSE / RPC_E_CHANGED_MODE just mean this
    // thread already has one.
    // Safety: plain COM initialisation on a thread we own.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

    let catalog = match PackageCatalog::OpenForCurrentUser() {
        Ok(c) => c,
        Err(e) => {
            log::warn!(
                "Could not open the package catalog for install events: {}",
                e
            );
            return;
        }
    };

    // Installing / Uninstalling / Updating fire repeatedly with progress;
    // only the final event changes what's launchable.
    let cb = on_change.clone();
    let installing = TypedEventHandler::<PackageCatalog, PackageInstallingEventArgs>::new(
        move |_, args: Ref<PackageInstallingEventArgs>| {
            if args.ok()?.IsComplete()? {
                cb();
            }
            Ok(())
        },
    );
    let cb = on_change.clone();
    let uninstalling = TypedEventHandler::<PackageCatalog, PackageUninstallingEventArgs>::new(
        move |_, args: Ref<PackageUninstallingEventArgs>| {
            if args.ok()?.IsComplete()? {
                cb();
            }
            Ok(())
        },
    );
    let cb = on_change.clone();
    let updating = TypedEventHandler::<PackageCatalog, PackageUpdatingEventArgs>::new(
        move |_, args: Ref<PackageUpdatingEventArgs>| {
            if args.ok()?.IsComplete()? {
                cb();
            }
            Ok(())
        },
    );
    // A package becoming disabled / tampered / re-enabled changes whether
    // it can launch.
    let cb = on_change;
    let status = TypedEventHandler::<PackageCatalog, PackageStatusChangedEventArgs>::new(
        move |_, _: Ref<PackageStatusChangedEventArgs>| {
            cb();
            Ok(())
        },
    );

    if let Err(e) = catalog.PackageInstalling(&installing) {
        log::warn!("Could not subscribe to package install events: {}", e);
        return;
    }
    for (what, result) in [
        ("uninstall", catalog.PackageUninstalling(&uninstalling)),
        ("update", catalog.PackageUpdating(&updating)),
        ("status", catalog.PackageStatusChanged(&status)),
    ] {
        if let Err(e) = result {
            log::warn!("Could not subscribe to package {} events: {}", what, e);
        }
    }
    log::info!("Watching the package catalog for app installs");

    // The subscriptions live as long as `catalog`; keep it (and this
    // thread) alive for the rest of the process.
    loop {
        std::thread::park();
    }
}

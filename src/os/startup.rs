// Cross-platform startup/autolaunch management

/// Check if the application is configured to start on login
pub fn get_startup_enabled() -> bool {
    crate::os::platform::startup::get_startup_enabled_impl()
}

/// Enable or disable starting the application on login
pub fn set_startup_enabled(enabled: bool) {
    crate::os::platform::startup::set_startup_enabled_impl(enabled);
}

/// One-shot migration to the platform's preferred autostart mechanism.
///
/// On Windows, autostart moved from the HKCU Run key (throttled and
/// staggered by Windows' startup queue — Kage appeared long after
/// logon) to a logon-triggered Scheduled Task. Users who enabled
/// autostart before that change still have the Run-key entry and would
/// never get the faster path unless they re-toggled the setting; this
/// upgrades them in place. No-op everywhere else, and when autostart
/// is off or already migrated. Spawns `schtasks` — call off the main
/// thread.
pub fn migrate_startup_mechanism() {
    #[cfg(target_os = "windows")]
    crate::os::platform::startup::migrate_startup_mechanism_impl();
}

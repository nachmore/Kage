// Cross-platform startup/autolaunch management

/// Check if the application is configured to start on login
pub fn get_startup_enabled() -> bool {
    #[cfg(target_os = "windows")]
    { crate::os::windows::startup::get_startup_enabled_impl() }

    #[cfg(target_os = "macos")]
    { crate::os::macos::startup::get_startup_enabled_impl() }

    #[cfg(target_os = "linux")]
    { crate::os::linux::startup::get_startup_enabled_impl() }
}

/// Enable or disable starting the application on login
pub fn set_startup_enabled(enabled: bool) {
    #[cfg(target_os = "windows")]
    { crate::os::windows::startup::set_startup_enabled_impl(enabled); }

    #[cfg(target_os = "macos")]
    { crate::os::macos::startup::set_startup_enabled_impl(enabled); }

    #[cfg(target_os = "linux")]
    { crate::os::linux::startup::set_startup_enabled_impl(enabled); }
}

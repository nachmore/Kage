// Cross-platform startup/autolaunch management

/// Check if the application is configured to start on login
pub fn get_startup_enabled() -> bool {
    crate::os::platform::startup::get_startup_enabled_impl()
}

/// Enable or disable starting the application on login
pub fn set_startup_enabled(enabled: bool) {
    crate::os::platform::startup::set_startup_enabled_impl(enabled);
}

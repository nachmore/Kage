//! Nothing beyond the filesystem watcher in `os::app_install_watch`: every
//! app source the launcher scans on this platform is a directory.

pub fn start_extra_impl(_on_change: crate::os::app_install_watch::ChangeCallback) {}

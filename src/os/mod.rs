// OS-specific functionality abstraction
// This module provides a unified interface for platform-specific operations

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

// Re-export platform-specific implementations with a common interface
// Note: These are available for advanced use cases, but most code should use
// the cross-platform functions exported below
#[cfg(target_os = "windows")]
#[allow(unused)]
pub use windows as platform;

#[cfg(target_os = "macos")]
#[allow(unused)]
pub use macos as platform;

#[cfg(target_os = "linux")]
#[allow(unused)]
pub use linux as platform;

// Common types and traits
pub mod cursor;
pub mod launcher;
pub mod process;
pub mod shell;

// Re-export common functionality
pub use cursor::get_cursor_position;
pub use launcher::{scan_applications, launch_application};
pub use process::{kill_process, configure_process_spawn};
pub use shell::{open_url, open_path};

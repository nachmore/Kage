//! Computer control module — accessibility-first desktop automation.
//!
//! Provides structured access to UI elements via OS accessibility APIs.
//! Platform-specific implementations live in `src/os/{platform}/accessibility.rs`.
//!
//! Primarily consumed by the `kage-computer-control-mcp` sidecar; the app
//! uses `tree` via `os::accessibility` for window-context capture.

pub mod script_runner;
pub mod tree;

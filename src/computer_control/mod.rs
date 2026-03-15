//! Computer control module — accessibility-first desktop automation.
//!
//! Provides structured access to UI elements via OS accessibility APIs.
//! Platform-specific implementations live in `src/os/{platform}/accessibility.rs`.
//!
//! Note: This module is primarily consumed by the `computer-control-mcp` binary,
//! not the main Tauri application.

pub mod tree;

// macOS-specific implementations.
//
// Submodules mirror the Windows set so the cross-platform `os::*`
// modules can dispatch uniformly via `crate::os::platform::<mod>`.
// `clipboard_history` is the only stub (descoped — macOS users rely on
// Paste/Maccy/Alfred); everything else has a native-API implementation.

pub mod accessibility;
pub mod ax_worker;
pub mod calendar;
pub mod clipboard;
pub mod clipboard_history;
pub mod cursor;
pub mod diagnostics;
pub mod file_search;
pub mod hotkey;
pub mod icon;
pub mod launcher;
pub mod power;
pub mod process;
pub mod shell;
pub mod startup;
pub mod user;
pub mod window_list;

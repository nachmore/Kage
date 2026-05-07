// Linux-specific implementations.
//
// Submodules mirror the Windows set so the cross-platform `os::*`
// modules can dispatch uniformly via `crate::os::platform::<mod>`. Where
// Linux doesn't have a native API yet (calendar, clipboard_history,
// file_search), the submodule is a stub returning empty results plus a
// once-per-process warn so users understand why nothing comes back.

pub mod cursor;
pub mod launcher;
pub mod process;
pub mod shell;
pub mod user;
pub mod clipboard;
pub mod clipboard_history;
pub mod calendar;
pub mod file_search;
pub mod startup;
pub mod hotkey;
pub mod icon;
pub mod window_list;
pub mod accessibility;
pub mod power;

# OS Abstraction Layer

Cross-platform OS code lives in `src/os/`. The pattern: a thin
cross-platform module per concern (cursor, clipboard, launcher, etc.) that
dispatches to a Windows/macOS/Linux implementation under it. The split is
compile-time `#[cfg(target_os = "...")]` — zero runtime overhead, no
dynamic dispatch.

Application code never reaches into a platform submodule directly. Always
go through the cross-platform API.

## Modules

```
src/os/
├── mod.rs
├── accessibility.rs
├── calendar.rs
├── clipboard.rs
├── clipboard_history.rs
├── cursor.rs
├── file_search.rs
├── hotkey.rs
├── icon.rs
├── launcher.rs
├── power.rs
├── process.rs
├── shell.rs
├── startup.rs
├── user.rs
├── window_list.rs
├── windows/      ← full implementation
├── macos/        ← common paths only; gaps return empty + warn once
└── linux/        ← common paths only; gaps return empty + warn once
```

Windows is the primary target and the only fully-featured implementation.
macOS and Linux cover the common paths used by features that ship cross-
platform; the rest are stubs that log a single warning per process and
return an empty result. The intent is "still compiles and runs" rather
than feature parity.

## Dispatch pattern

Every cross-platform module follows the same shape — Pattern A in the
audit. The cross-platform fn takes care of dispatch; the platform fn is
named with an `_impl` suffix to make ownership obvious at the call site.

```rust
// src/os/clipboard.rs
pub fn get_clipboard_text() -> Option<String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::clipboard::get_clipboard_text_impl() }
    #[cfg(target_os = "macos")]
    { crate::os::macos::clipboard::get_clipboard_text_impl() }
    #[cfg(target_os = "linux")]
    { crate::os::linux::clipboard::get_clipboard_text_impl() }
}
```

```rust
// src/os/windows/clipboard.rs
pub fn get_clipboard_text_impl() -> Option<String> { /* Win32 calls */ }
```

```rust
// src/os/macos/clipboard.rs — stub
pub fn get_clipboard_text_impl() -> Option<String> {
    crate::os::macos::warn_once("clipboard::get_clipboard_text");
    None
}
```

For one-line helpers (e.g. `is_dark_mode`, `fonts_dir`) the cross-platform
file may keep the `#[cfg]` switch inline rather than going through `_impl`
fns. Anything non-trivial gets the full pattern so platform implementations
stay independently testable and easy to grep.

## Adding a new operation

1. Create or update `src/os/<concern>.rs` with the public API. Use the
   dispatch pattern above.
2. Add a matching `<concern>.rs` (or extend the existing one) in
   `src/os/windows/`, `src/os/macos/`, `src/os/linux/`. Each must export
   the same `_impl` function, even if the macOS/Linux version is a stub
   that calls `warn_once` and returns a default.
3. Re-export from `src/os/mod.rs` so call sites can do
   `use crate::os; os::your_function()`.
4. Tests live next to the implementation. Pure helpers (e.g. parsers,
   format converters) get unit tests without going through the whole
   stack — see `is_strict_iso_date` in `os/windows/calendar.rs` as a
   reference.

## Things that aren't in `os/` and shouldn't be

- **Tauri-typed code** (windows, app handles, IPC commands) lives in
  `commands/` and `setup/`. When pure logic gets entangled there, lift it
  into a sibling module (the `chunk_batcher.rs` precedent) so it can be
  tested without standing up a Tauri AppHandle.
- **The MCP binary** (`src/bin/computer_control_mcp.rs`) reaches into
  `os/` directly because it's a separate binary, not "application code"
  in the kage sense. The same dispatch contract applies — it consumes
  the cross-platform API, never the platform submodules.

## Capabilities and feature gating

Stubs return defaults and warn so missing features don't crash. UI that
shouldn't appear on platforms that don't implement a feature should query
the relevant config flag rather than probing the OS layer at runtime.
There's no central capabilities struct yet (audit P3.8 follow-up); add
one if/when the UI starts needing to hide entry points.

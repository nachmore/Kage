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
├── login_env.rs    ← login-shell environment for user-tool spawns (see below)
├── permissions.rs  ← macOS TCC status + repair; "not applicable" elsewhere
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

## Spawning user-installed tools (`login_env`)

A Kage launched from Finder / Dock / `open` (including the post-update
relaunch) inherits launchd's minimal environment —
`PATH=/usr/bin:/bin:/usr/sbin:/sbin`, nothing from `.zshrc`. Linux desktop
sessions are often similar. Anything Kage spawns inherits that, and so does
everything the agent spawns: an `mcp.json` entry like
`"command": "builder-mcp"` (in `~/.toolbox/bin`) fails to start even though
it works in a terminal.

`os::login_env` fixes this the way VS Code's "resolve shell environment"
does, but more conservatively:

- `prewarm()` (called first thing in `app_setup::configure`) runs
  `$SHELL -i -l` once on a background thread — falling back to the passwd
  record's shell, then `/bin/sh` — and captures `/usr/bin/env -0` between
  per-run markers. 10 s hard timeout that SIGKILLs the shell's whole
  process group; own session (no controlling terminal); stdin `/dev/null`;
  retries as a non-interactive login shell if an interactive rc `exec`s
  away. bash/zsh/sh/dash/ksh/fish/nu/csh/tcsh/pwsh are handled.
- `apply(&mut cmd)` overlays the result onto one child: **PATH merged**
  (login entries first, then inherited extras), **other variables
  add-missing only** (nothing Kage or launchd set is overridden), an
  **explicit denylist** (`HOME`, `TMPDIR`, `USER`, `SSH_*`, `__CF*`,
  `XPC_*`, `KAGE_*`, terminal and shell-session state, …), size caps, and
  anything already set on the `Command` wins. It waits up to 5 s for an
  in-flight probe — never on the main thread — else the child gets the
  inherited PATH plus well-known user bin dirs. `overlay(&[..])` returns the
  same pairs for env-list APIs (the kage-core script runner).
- Bare program names resolve too: `std::process::Command` looks the program
  up in the PATH set *on the Command* on Unix.

Call `apply` for every spawn of a user tool — the agent, `which` lookups
that mirror it, version probes, npm, python, icalBuddy, shortcuts, proxy
scripts — after the caller's own `cmd.env(...)`. Don't call it for OS
binaries (`osascript`, `open`, `tccutil`, `codesign`, …).

Launched from a terminal (any stdio stream is a TTY) nothing is probed or
changed: Kage already has the user's real environment. Override with
`KAGE_LOGIN_SHELL_ENV=0` (never) or `=1` (always). rc files can test
`KAGE_RESOLVING_ENVIRONMENT=1` to skip terminal-only work (`exec tmux`).
The log records the shell, timing, PATH, and the *names* of imported
variables (never their values). Manual end-to-end check:
`tests/login_env_e2e_test.rs`.

macOS and Linux share one POSIX implementation (`src/os/login_env/`);
Windows is a no-op — Explorer-launched processes already get the full
user + system environment.

## Things that aren't in `os/` and shouldn't be

- **Tauri-typed code** (windows, app handles, IPC commands) lives in
  `commands/` and `setup/`. When pure logic gets entangled there, lift it
  into a sibling module (the `chunk_batcher.rs` precedent) so it can be
  tested without standing up a Tauri AppHandle.
- **The MCP binary** (`computer_control_mcp/src/main.rs`) reaches into
  `os/` directly because it's a separate binary, not "application code"
  in the kage sense. The same dispatch contract applies — it consumes
  the cross-platform API, never the platform submodules.

## Capabilities and feature gating

Stubs return defaults and warn so missing features don't crash. UI that
shouldn't appear on platforms that don't implement a feature should query
the relevant config flag rather than probing the OS layer at runtime.
There's no central capabilities struct yet (audit P3.8 follow-up); add
one if/when the UI starts needing to hide entry points.

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
├── app_uninstall.rs ← launcher "Uninstall…": Trash the .app / run the app's own uninstaller
├── calendar.rs
├── clipboard.rs
├── clipboard_history.rs
├── cursor.rs
├── file_ops.rs     ← files on the clipboard, Trash / Recycle Bin, run-as-admin, Properties
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
`"command": "my-mcp-server"` (in `~/.local/bin`) fails to start even though
it works in a terminal.

`os::login_env` fixes this the way VS Code's "resolve shell environment"
does, but more conservatively:

- `prewarm()` (called first thing in `app_setup::configure`, right after
  `set_enabled(config.acp.agent.import_login_shell_env)`) runs
  `$SHELL -i -l` once on a background thread — falling back to the passwd
  record's shell, then `/bin/sh` — and captures `/usr/bin/env -0` between
  per-run markers (plain `/usr/bin/env`, parsed line by line, where `-0`
  is missing: macOS 11 and 12.0–12.2). 10 s hard timeout; on expiry every
  process in the probe's *session* is SIGKILLed (process group first, then
  a session sweep, because rc-file job control gives each job its own
  group). Own session (no controlling terminal); stdin `/dev/null`;
  retries as a non-interactive login shell if an interactive rc `exec`s
  away. bash/zsh/sh/dash/ksh/fish/nu/csh/tcsh/pwsh are handled.
- `apply(&mut cmd)` overlays the result onto one child: **PATH merged**
  (login entries first, then inherited extras, then existing well-known
  user bin dirs — always, since a probe can succeed with a degraded PATH),
  **other variables add-missing only** (nothing Kage or launchd set is
  overridden), an **explicit denylist** (`HOME`, `TMPDIR`, `USER`, `SSH_*`
  except a non-`ssh-agent` `SSH_AUTH_SOCK`, `__CF*`, `XPC_*`, `KAGE_*`,
  `GPG_TTY`, `BASH_ENV`/`ENV`, `LD_PRELOAD`/`LD_AUDIT`, terminal and
  shell-session state, …), **no temporary AWS credentials** (a session
  token drops key/secret/token/expiry; `AWS_PROFILE`/`AWS_REGION` stay),
  size caps, and anything already set on the `Command` wins.
  `LD_LIBRARY_PATH`, `NODE_OPTIONS`, `PYTHONPATH` *are* imported, for
  terminal parity. `overlay(&[..])` returns the same pairs for env-list
  APIs (the kage-core script runner). The agent spawn uses `apply_agent`,
  which additionally refreshes a result older than 30 min in the
  background.
- **Blocking.** `apply`/`overlay` wait for the *first* probe only, never
  on the main thread, and only until 5 s after it started (one deadline
  shared by all callers). That can still be seconds early in a launch, so
  **call them from a blocking context** — `spawn_blocking` or a std
  thread — never inside an `async` body on a Tokio worker (Tauri `async`
  commands, `tauri::async_runtime::spawn`). Re-probes (one background
  retry a minute after a failure, backing off to 30 min; the 30-min agent
  refresh) are never waited for. The agent transport calls `wait_ready()`
  before taking its connect lock, so the lock never covers the wait.
- Bare program names resolve too: `std::process::Command` looks the program
  up in the PATH set *on the Command* on Unix.

Call `apply` for every spawn of a user tool — `which` lookups that mirror
the agent, version probes, npm, python, icalBuddy, shortcuts, proxy
scripts — after the caller's own `cmd.env(...)`. Don't call it for OS
binaries (`osascript`, `open`, `tccutil`, `codesign`, …).

Switches: **Settings → Agents → "Use my shell's environment for agent
tools"** (`acp.agent.import_login_shell_env`, default on; `set_enabled` is
called at startup and from `apply_runtime_config` on every save/import, so
a toggle applies to the next spawn without a restart). Launched from a
terminal (any stdio stream is a TTY) nothing is probed or changed: Kage
already has the user's real environment. `KAGE_LOGIN_SHELL_ENV=0` (never)
or `=1` (always) overrides both. Unit tests (`cfg(test)`) never probe
unless `KAGE_LOGIN_SHELL_ENV=1`; the mock-app integration harness calls
`set_enabled(false)`. rc files can test `KAGE_RESOLVING_ENVIRONMENT=1` to
skip terminal-only or side-effecting work (user-facing write-up:
[`AGENT_ENVIRONMENT.md`](AGENT_ENVIRONMENT.md)). The log records the
shell, timing, PATH, and the *names* of imported variables (never their
values), and one hint line when a probe takes over 3 s or is killed.
Manual end-to-end check: `tests/login_env_e2e_test.rs`.

macOS and Linux share one POSIX implementation (`src/os/login_env/`:
`policy` = what crosses over, `probe` = running the shell, `schedule` =
when to re-probe and who may wait); Windows is a no-op — Explorer-launched
processes already get the full user + system environment.

## Destructive file operations (`file_ops`, `app_uninstall`)

The launcher's result menu can move items to the Trash / Recycle Bin, cut
them (Windows), and move an app bundle to the Trash (macOS Uninstall). Each
of those entry points takes a `Preflight` — the caller's re-check that the
path still names the item it vetted (same canonical path, same file
identity; `launcher_actions::Resolved::verify_unchanged`). Platform code
must run it **on the thread that makes the destructive call, immediately
before it**, and abort with nothing done on `Err`: on Windows that's inside
the `with_com_sta` shell thread right before `SHFileOperationW`, and with
the clipboard already open right before `EmptyClipboard` for a Cut. Never
check on one thread and hand the call to another. An item whose identity
can't be read (then or now) is refused — `None == None` is not "unchanged".

What the check can't do, because the OS APIs take paths, not handles:

- It narrows the window between check and call to microseconds; it can't
  close it.
- A Cut is pasted later by Explorer from the `CF_HDROP` path list —
  path-based by design. Kage checks the item when it publishes the list;
  what the path names at paste time is the file manager's to resolve.
- Linux trashes through `gio` / `trash-cli` / `kioclient`, separate
  processes handed the path.
- File identity is (volume, file index). FAT and exFAT have no stable file
  index — Windows derives it from where the directory entry sits — so a
  replacement that reuses the entry can carry the same identity there, and
  on ReFS (Dev Drives) the 64-bit index isn't guaranteed unique; the
  canonical-path half of the check still applies.
- `SHFileOperationW` is limited to `MAX_PATH`; longer paths are refused up
  front and the menu lists Recycle Bin disabled, saying why.

Windows Uninstall runs an uninstaller only from an allowlist
(`app_uninstall::plan_uninstall`): `msiexec.exe /x {ProductCode}` for a
Windows Installer product, or the product's own `.exe` from its verified
install folder with no argument or one of `/uninstall`, `--uninstall`,
`-uninstall`, `/u`. Anything else opens Settings → Installed apps. Don't
add switch parsing; extend the allowlist only with forms known to be
interactive.

A shared folder (Program Files, the profile, AppData, Downloads, temp, …)
is never one product's folder, however it's spelled. `WinFs::sharing`
reads a folder as written and as `canonicalize` resolves it (8.3 names,
junctions, trailing dots and spaces — `Downloads.` is Downloads), and a
resolution is one of three things: resolved, missing (`NotFound`), or
unknown (any other error). Sharing fails closed: a folder is `NotShared`
only if it resolves, no shared folder resolved unknown (missing ones are
compared as configured), and neither form is shared. Only a `NotShared`
folder (`WinFs::is_unshared`) can verify an uninstaller — `plan_uninstall`
requires it of the install folder and of the folder the uninstaller
resolves into, and runs nothing that resolves to a network share — or
become a protected product tree (`product_install_dirs`). Anything else
opens Settings and protects no tree. `WinFs::is_shared` (known shared) only
discards matching evidence. Compare Windows paths for these decisions
through `WinFs`, never as raw strings.

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

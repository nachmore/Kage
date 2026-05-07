# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

**Kage** — cross-platform desktop AI assistant built on Tauri 2.10. Rust backend, vanilla HTML/CSS/JS frontend (no framework). Talks to a separate `kage-cli` agent backend over ACP (Agent Communication Protocol).

## Build & Run

```bash
cargo tauri dev -- /dev          # dev mode (DevTools, tray reload, RUST_BACKTRACE=1)
cargo tauri dev -- /debug        # ACP protocol message logging to stdout
cargo tauri dev -- /dev /debug   # both
cargo tauri build                # release + NSIS installer (target/release/bundle/nsis/)
cargo build                      # debug binaries only — no installer, no bundling
```

`cargo build --release` does **not** produce the installer; only `cargo tauri build` does.

### Two binaries — they don't rebuild together

`src/main.rs` → `kage` (the app). `src/bin/computer_control_mcp.rs` → `kage-computer-control-mcp` (standalone MCP server spawned by kage-cli over stdio).

`cargo tauri dev` and `cargo check` rebuild **only** `kage`. After editing `src/bin/computer_control_mcp.rs` or any module it pulls in (notably `src/os/accessibility.rs`, `src/computer_control/`):

```bash
cargo build --bin kage-computer-control-mcp
```

If the binary is locked because it's running, kill it first (`Get-Process -Name kage-computer-control-mcp | Stop-Process -Force`), then rebuild and restart the app so kage-cli picks up the new binary.

`cargo tauri build` rebuilds it automatically (see `tauri.conf.json` → `beforeBuildCommand`).

## Test & Lint

```bash
cargo test                          # all Rust tests; parallelism capped at jobs=2 in .cargo/config.toml to avoid paging-file exhaustion
cargo test --test acp_client_test   # single integration test file
cargo test test_name                # single test by name
cd ui/tests && npm install          # first time only
cd ui/tests && npx vitest run       # JS tests for ui/js/shared/*
cd ui/tests && npx vitest run path/to/file.test.js  # single JS test file
python scripts/test_all.py          # everything (Rust + JS) in one go

cargo check                         # fast type/borrow check
cargo fmt
cargo clippy
```

Tauri-dependent modules (`automation`, `commands`, `setup`, `single_instance`, `state`, `tray`, `updater`) are gated `#[cfg(not(test))]` in `lib.rs` — Tauri's type system doesn't compile under `--test`. Tests cover the pure modules only.

## Architecture

### Process & lifecycle (`src/main.rs`)

1. `single_instance::try_acquire` — OS-level file lock. Second instance signals the running one over TCP and exits.
2. `panic_handler::install` — captures panics into `crash.log` before logger init.
3. `logger::init_logger` then `app_log::init` (in-memory ring buffer surfaced in the About settings).
4. On Windows, `os::windows::process::install_kill_on_exit_job` creates a Job Object so children (TTS server, kage-cli, MCP servers) die with the parent.
5. `AcpClient::new(mode)` — connection mode chosen from config (`stdio`/`pipe`/`tcp`).
6. `AppState` (`src/state.rs`) is the single shared struct passed via `tauri::Builder::manage`. Most fields are `Arc<Mutex<…>>`. Frontend talks to backend through Tauri commands registered in `tauri::generate_handler!` — every public command must be added there or it won't be callable.

### ACP client (`src/acp_client/`)

`mod.rs` is the public surface. `transport.rs` handles the stdio/pipe/tcp framing. `session.rs` tracks per-session state. `types.rs` is the JSON-RPC schema. The notification handler is wired up in `setup` (`commands::messaging::setup_notification_handler`) and routes streaming updates to the frontend via Tauri events.

### OS abstraction (`src/os/`)

Cross-platform API lives in `src/os/<module>.rs`. Per-platform impls are in `src/os/windows/`, `src/os/macos/`, `src/os/linux/`. Selection is compile-time via `#[cfg(target_os = "...")]` in `mod.rs`. **Never import a platform-specific module directly from app code** — always go through the abstraction layer. Windows is fully implemented; macOS/Linux cover only common paths.

### Frontend (`ui/`)

Five Tauri windows defined in `tauri.conf.json`: `main` (chat sessions), `floating`, `settings`, `context-menu`, `inline-assist`. Each loads its own HTML and JS entry point.

JS is split:
- `ui/js/shared/` — used by multiple windows. Anything reused belongs here, never duplicated.
- `ui/js/floating/`, `ui/js/chat/`, `ui/js/settings/` — window-specific.
- `ui/js/extension-sandbox/` — the iframe sandbox host code.

CSS variables and shared components live in `ui/css/shared-kage-tokens.css` and `ui/css/shared-components.css`. Both must be loaded in every window's HTML.

Vendor JS (marked, mermaid, prismjs, mathjs, graphviz wasm) is npm-managed in `ui/vendor/` and loaded via `<script>` tags from `ui/vendor/lib/`, not ES module imports.

### Settings window pattern (`ui/js/settings/`)

Each section is a class extending `SettingsModule` (`base.js`), registered in `manager.js` in sidebar order. Sidebar items use `data-section` matching the module `id`. Modules implement `render()`, `load(config)`, `save(config)`, `validate()`, optionally `initialize()` and `destroy()`. Use `createCheckboxRow()` / `createControlRow()` from the base for consistent layout.

When parsing markdown via `marked.parse()`, sanitize first — if input is HTML, marked passes it through raw and `<style>`/`<script>` will corrupt the page.

### Extension sandbox (`docs/SECURITY_MODEL.md`)

Every extension — bundled or user-installed — is **untrusted**. Provider code runs in an iframe with `sandbox="allow-scripts"` (no `allow-same-origin`), so it has a unique null origin and no access to `window.__TAURI__`. The host (`ui/js/shared/extension-sandbox-host.js`) authoritatively enforces capability checks on every IPC `invoke()` arriving over the single `MessagePort`. Vendor libs are allow-listed by name in `extension-manager.js` and run in a terminable Web Worker.

CSP is intentionally `null` in `tauri.conf.json` — the trust boundary is the tool permission system, not the WebView CSP. Don't add features that load external/untrusted web content without revisiting this.

### Configuration

JSON in user config dir (`%APPDATA%/kage/config.json` etc.). Loaded by `Config::load`, migrated by `config_migrations`. **Every field must have `#[serde(default)]`** — old configs must keep loading after a schema change. Updates broadcast a `config_updated` Tauri event; all windows listen and reapply theme/hotkeys from there.

Extensions persist data via `save_extension_data` / `load_extension_data` Tauri commands (JSON in `config_dir/extension-data/`). **Never use `localStorage`** — WebView2 can wipe it on update or reinstall.

### Logging

Use `log::*` macros (`info!`, `warn!`, `error!`, `debug!`). Avoid `println!` outside the startup banner. Both the `log` crate and direct `app_log::log` calls funnel through the same writer thread to a JSONL file (`%LOCALAPPDATA%\kage\logs\app.jsonl` on Windows) plus an in-memory ring buffer shown in the About settings. `/debug` flag adds detailed ACP protocol logging to stdout.

## Conventions

- **Build output**: when a build/test fails, read the **full** output, not just the last 30 lines. Errors and warnings can appear anywhere.
- **Inclusive language**: don't use master/slave/whitelist/blacklist. See `~/.claude/rules/amazon-builder-context-do-not-delete.md` for the substitutions table.
- **Commits**: never commit unless the user explicitly says "commit". Don't commit on task completion or on "go ahead" / "do it".

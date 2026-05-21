# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

**Kage** — cross-platform desktop AI assistant built on Tauri 2.10. Rust backend, vanilla HTML/CSS/JS frontend (no framework). Talks to a separate `kage-cli` agent backend over ACP (Agent Communication Protocol).

## Build & Run

```bash
cargo tauri dev -- /dev          # dev mode (DevTools, tray reload, RUST_BACKTRACE=1)
cargo tauri dev -- /debug        # ACP protocol message logging to stdout
cargo tauri dev -- /dev /debug   # both
cargo tauri build                # platform-native installer/bundle (Windows: NSIS; macOS: .app + .dmg)
cargo build                      # debug binaries only — no installer, no bundling
```

`cargo build --release` does **not** produce the installer or embed the frontend; only `cargo tauri build` does. A binary from `cargo build` will fail at runtime with `ERR_CONNECTION_REFUSED` because it still expects the dev server at `localhost:1420`. Use `cargo tauri build --no-bundle` if you want a standalone exe without the NSIS/DMG step. `cargo check` and `cargo build` are still the right commands for fast type/borrow validation during Rust iteration.

### Two binaries — built separately, chained automatically

`src/main.rs` → `kage` (the app). `src/bin/computer_control_mcp.rs` → `kage-computer-control-mcp` (standalone MCP server spawned by kage-cli over stdio).

`cargo tauri dev` rebuilds the MCP binary first (chained through `scripts/dev_server.py` → `build_mcp_binary()`), then builds and runs `kage`. Plain `cargo check` and `cargo build` only touch `kage`, so if you're iterating with those without `cargo tauri dev`, after editing `src/bin/computer_control_mcp.rs` or any module it pulls in (notably `src/os/accessibility.rs`, `src/computer_control/`) run:

```bash
cargo build --bin kage-computer-control-mcp
```

If the binary is locked because it's running, kill it first (Windows: `Get-Process -Name kage-computer-control-mcp | Stop-Process -Force`; macOS/Linux: `pkill -f kage-computer-control-mcp`), then rebuild and restart the app so kage-cli picks up the new binary.

`cargo tauri build` rebuilds it automatically too (see `tauri.conf.json` → `beforeBuildCommand`).

To skip the MCP rebuild during dev (purely UI/main-binary iteration), pass `--no-mcp-build` to the dev server, e.g. by editing the `beforeDevCommand` line in `tauri.conf.json` for that session.

## Test & Lint

```bash
cargo test                          # all Rust tests; parallelism capped at jobs=2 in .cargo/config.toml to avoid paging-file exhaustion
cargo test --test acp_client_test   # single integration test file
cargo test test_name                # single test by name
cd ui-tests && npm install          # first time only
cd ui-tests && npx vitest run       # JS tests for ui/js/shared/*
cd ui-tests && npx vitest run path/to/file.test.js  # single JS test file
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

Vendor JS (marked, mermaid, prismjs, mathjs, graphviz wasm) is npm-managed in `ui-vendor/` (outside `ui/` so npm tooling doesn't get embedded into the binary). Browser bundles get copied into `ui/vendor/lib/` by `ui-vendor/setup.js` and loaded via `<script>` tags, not ES module imports.

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


## Telemetry

Anonymous product analytics via [Aptabase](https://aptabase.com). See `docs/PRIVACY.md` for the user-facing policy.

### Setting the Aptabase key

The key is not secret (it appears in outbound network traffic), but we keep it out of source so third-party forks don't send events to our dashboard.

Resolution order (first match wins):
1. `APTABASE_KEY` env var — set this in CI from a repo secret.
2. `.aptabase-key` file at the repo root — gitignored, used for local release builds. Copy `.aptabase-key.example` and paste your key.

If neither is set, the plugin is never registered and `telemetry::track()` is a no-op. Dev builds without a key still work — everything else just runs quietly.

### Implementation

Lives in `src/telemetry.rs`. Every call site goes through `telemetry::track(&app, "event_name", props)` which short-circuits when:
- The build has no key (via `option_env!` in `src/telemetry.rs`).
- The user has opted out (`config.telemetry.enabled == false`) or not consented yet (`install_id == None`).

The plugin is only registered in `main.rs` when the key exists, so disabled builds don't ship the background worker.

Events fire from:
- Startup (`main.rs` after `.build()`): `app_installed` | `app_upgraded` | `app_started`, plus `app_daily_active` once per UTC day.
- Shutdown (`.run(|h, event|)` → `RunEvent::Exit`): `app_exited` with a blocking flush.
- Specific command handlers (`execute_shortcut`, `commit_extension_install`, `open_settings_window`, …).
- Frontend via `ui/js/shared/telemetry.js` → `trackEvent(name, props?)` → `telemetry_track` command. Allow-listed event names live in `KNOWN_EVENTS` in that file.

Adding a new event:
1. Add the name to `KNOWN_EVENTS` in `ui/js/shared/telemetry.js` (or none — the list is advisory).
2. Ensure props are string/number only. Bucket lengths, never send raw text or paths.
3. Update `docs/PRIVACY.md` if the disclosure list needs to change.

Settings → Privacy (`ui/js/settings/privacy.js`) lets users toggle and reset their install ID. The welcome screen's privacy step is the initial opt-out surface; `complete_first_run` records their decision via `telemetry::set_consent`. The `v2 → v3` migration explicitly disables telemetry for existing users so they aren't auto-opted-in silently.


## Auto-updates

Signed in-app updates via `tauri-plugin-updater`. See `docs/RELEASE.md` for the full release + signing flow.

Key points for engineers touching the updater:

- Three channels: `stable`, `beta`, `dev`. Endpoint URLs per channel live in `Cargo.toml [package.metadata.update]`; build.rs exposes them as compile-time env vars; `src/updater.rs::endpoint_for_channel` routes.
- The private signing key lives only in CI (GitHub Actions secret `TAURI_SIGNING_PRIVATE_KEY`). The matching public key is baked into every binary via `build.rs` (from `.tauri-updater-pubkey` file or `TAURI_UPDATER_PUBKEY` env). Release builds fail loudly if no pubkey is configured.
- The plugin handles: manifest fetch, signature verification, download, and per-platform install + relaunch. `src/updater.rs` wraps it with our scheduling layer (daily check, 5-minute-idle gate for silent installs) and session-resume-after-update (`last-session.txt` handoff via `startup::resolve_resume_session_id`).
- Never call `run_installer` — the plugin owns that now. Deleted from `src/os/mod.rs` in the migration.
- `VALID_CHANNELS` in `src/updater.rs` is the authority; the JS dropdown in `ui/js/settings/updates.js` mirrors the list and normalises unknown values to stable. `save_config` also normalises on the way in so a hand-edited config.json can't trap a user on a dead channel.

Adding a new channel:
1. Add entry to `[package.metadata.update]` and `[package.metadata.update.dev]` in `Cargo.toml`.
2. Add the corresponding `UPDATE_ENDPOINT_<NAME>` handling to `build.rs`.
3. Add to `VALID_CHANNELS` in `src/updater.rs` and a match arm in `endpoint_for_channel`.
4. Add a label entry in the `_renderChannelOptions` map in `ui/js/settings/updates.js` (the channel list itself is fetched from Rust via `get_app_info`).
5. Add the CI workflow trigger in `.github/workflows/release.yml`.
6. **Decide migration policy**: should existing users move to the new channel, or stay where they are until they opt in? The `v3→v4` migration set the precedent — default existing configs to `stable`. Adding a new channel almost always means "leave existing users alone" (they didn't ask for it), so usually no migration is needed. The exception is if you're *splitting* an existing channel — then you need a migration to redirect users to the appropriate replacement.

Removing the update plugin: the easiest partial rollback is to `option_env!("TAURI_UPDATER_PUBKEY")` → `None`, which makes `plugin_check` a no-op and quietly disables updates. Full removal requires dropping the plugin registration in `main.rs` and the `updater:default` capability — but don't unless you're sure there's no MITM-safe alternative.

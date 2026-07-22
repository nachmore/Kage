# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

**Kage** — cross-platform desktop AI assistant built on Tauri 2.10. Rust backend, vanilla HTML/CSS/JS frontend (no framework). Talks to a separate agent backend (e.g. `kiro-cli`) over ACP (Agent Communication Protocol).

## Build & Run

```bash
cargo tauri dev -- /dev          # dev mode (DevTools, tray reload, RUST_BACKTRACE=1)
cargo tauri dev -- /debug        # ACP protocol message logging to stdout
cargo tauri dev -- /dev /debug   # both

# Fast dev build (USE THESE for hand-off-the-binary iteration):
pwsh scripts/build_dev_installer.ps1            # Windows: debug profile, full installer
pwsh scripts/build_dev_installer.ps1 -NoBundle  # Windows: just kage.exe, no NSIS step
pwsh scripts/build_dev_installer.ps1 -Replace   # Build, kill running kage, swap installed exe
pwsh scripts/build_dev_installer.ps1 -Release   # Release profile (slower compile, faster runtime)
bash scripts/build_dev_installer.sh             # macOS/Linux equivalent
bash scripts/build_dev_installer.sh --no-bundle
bash scripts/build_dev_installer.sh --replace   # macOS/Linux: same hot-swap flow
bash scripts/build_dev_installer.sh --release   # Release profile

# Ship-quality build (slow — full LTO, single codegen unit):
cargo tauri build                # ~13 min on Windows; CI uses this for releases
cargo build                      # debug binaries only — no installer, no bundling
```

**Default for dev iteration: `scripts/build_dev_installer.{ps1,sh}`** — builds with `cargo tauri build --debug` (debug profile). Smallest compile time, unoptimised runtime that's fine for testing, and `cfg(debug_assertions)`-keying dependencies (notably `tauri-plugin-aptabase`) tag every event `isDebug=true` so Aptabase routes them into the Debug bucket and your prod dashboard stays clean. Pass `-Release` / `--release` to use the relaxed release profile (`CARGO_PROFILE_RELEASE_LTO=false`, `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`) when you need to verify perf or repro a release-only bug — that path produces production-classified telemetry. Add `-NoBundle` / `--no-bundle` either way to skip NSIS bundling and produce just the `kage.exe` (under `target/debug/` or `target/release/` depending on profile).

`cargo build --release` does **not** produce the installer or embed the frontend; only `cargo tauri build` does. A binary from `cargo build` will fail at runtime with `ERR_CONNECTION_REFUSED` because it still expects the dev server at `localhost:1420`. `cargo check` and `cargo build` are still the right commands for fast type/borrow validation during Rust iteration.

### Two binaries — built separately, chained automatically

`src/main.rs` → `kage` (the app). `computer_control_mcp/src/main.rs` → `kage-computer-control-mcp` (standalone MCP server spawned by the agent backend over stdio).

**The MCP sidecar is self-provisioned by `build.rs`.** Every cargo build of the main crate (`cargo build`, `cargo tauri dev/build` — anything that runs `build.rs`) rebuilds the sidecar into `target/sidecar-build/` (separate target dir — a nested cargo build into the parent's tree would deadlock on cargo's lock) and stages it at `src-tauri/binaries/kage-computer-control-mcp-<triple>[.exe]`, where `tauri_build::build()` validates it and copies it next to the main binary in `target/<profile>/`. `rerun-if-changed` on `computer_control_mcp/` and `kage-core/` means an edit there re-runs the provisioning on the next build; a warm no-op costs under a second. There is no separate build step, no placeholder file, and no way to bundle a stale sidecar.

If the staging copy fails because the binary is locked by a running instance, kill it first (Windows: `Get-Process -Name kage-computer-control-mcp | Stop-Process -Force`; macOS/Linux: `pkill -f kage-computer-control-mcp`), rebuild, and restart the app so the agent backend picks up the new binary.

Note: `cargo check` / `cargo clippy` / `cargo test` of the main crate also run `build.rs`, so they provision the sidecar too — that's expected and cheap on a warm tree.

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

All modules compile under `--test`, including Tauri-dependent ones (`automation`, `commands`, `setup`, `state`, `tray`, `updater`). Inline tests in those modules should stay pure-logic — anything that needs a real `AppHandle`, `WebviewWindow`, or live updater plugin belongs in `tests/` so it can stand up the runtime.

## Architecture

### Process & lifecycle (`src/main.rs`)

1. `tauri_plugin_single_instance` — the plugin's setup hook runs in the second instance, signals the running one (over the plugin's named-pipe / AF_UNIX IPC), forwards its argv/URL, and exits.
2. `panic_handler::install` — captures panics into `crash.log` before logger init.
3. `logger::init_logger` then `app_log::init` (in-memory ring buffer surfaced in the About settings).
4. On Windows, `os::windows::process::install_kill_on_exit_job` creates a Job Object so children (TTS server, agent backend, MCP servers) die with the parent.
5. `AcpClient::new(mode)` — connection mode chosen from config (`stdio`/`pipe`/`tcp`).
6. `AppState` (`src/state.rs`) is the single shared struct passed via `tauri::Builder::manage`. Most fields are `Arc<Mutex<…>>`. Frontend talks to backend through Tauri commands registered in `tauri::generate_handler!` — every public command must be added there or it won't be callable.

**Every `#[tauri::command]` returns `Result<T, AppError>` (never `Result<T, String>`).** `AppError` (`src/error.rs`) serializes as `{ kind, message }` so the frontend can pattern-match on error categories (`connection_lost`, `rate_limited`, etc.). The `tests/command_error_type_parity_test.rs` integration test enforces this — a new command that returns `String` errors will fail CI. JS callers route errors through `errMessage(e)` / `errLabel(label, e)` from `ui/js/shared/error-message.js`, never `'X: ' + e` (which produces `"X: [object Object]"` for AppError objects).

**Register state on the Builder, not inside `setup()`.** Tauri starts loading window webviews — and the JS inside them — as soon as the Builder constructs them, well before the `setup()` closure runs. Frontend invokes that hit `tauri::State<...>` will fail with "state not managed" until ~5 seconds into startup if state registration is deferred to `setup()`. Construct cheap state up front and `.manage()` it on the Builder; reserve `setup()` for work that genuinely needs `&mut App` / `app.handle()` (notification handler wiring, tray construction, hotkey registration, listener installs). See `src/main.rs::run` for the canonical layout.

### ACP client (`src/acp_client/`)

`mod.rs` is the public surface. `transport.rs` handles the stdio/pipe/tcp framing. `session.rs` tracks per-session state. `types.rs` is the JSON-RPC schema. The notification handler is wired up in `setup` (`commands::messaging::setup_notification_handler`) and routes streaming updates to the frontend via Tauri events.

**Vendor extensions: `_kage.dev/*` and `_kiro.dev/*` are interchangeable.** Two ACP vendor namespaces are recognised; the extension surface (`commands/available`, `commands/execute`, `metadata`, `compaction/status`, `error/rate_limit`) is identical across both prefixes. Match incoming notifications by *suffix* via `acp_client::vendor_method_suffix`. For outgoing requests, build the method name with `client.vendor_method("commands/execute")` so it targets whichever prefix the agent has been observed using (pinned on first inbound notification, defaults to `_kage.dev/`).

### OS abstraction (`src/os/`)

Cross-platform API lives in `src/os/<module>.rs`. Per-platform impls are in `src/os/windows/`, `src/os/macos/`, `src/os/linux/`. Selection is compile-time via `#[cfg(target_os = "...")]` in `mod.rs`. **Never import a platform-specific module directly from app code** — always go through the abstraction layer. Windows is fully implemented; macOS/Linux cover only common paths.

### Frontend (`ui/`)

Three Tauri windows preloaded via `tauri.conf.json`: `main` (chat sessions), `floating`, `inline-assist`. `settings` and `context-menu` are built on demand via `WebviewWindowBuilder` from `commands::window` — kept out of the initial windows array so we don't pay for their WebView2 process at every launch. `welcome` and `store` are also on-demand. Each window loads its own HTML and JS entry point.

**Release builds embed `ui/` via brotli — don't grep the .exe for source strings to verify a build.** `tauri::generate_context!()` runs `tauri-codegen`, which brotli-compresses every file under `frontendDist` into the binary's `.rdata`. A marker like `[CHAT] foo` will never match `findstr` on `kage.exe` even when the latest source IS embedded; you must decompress the codegen cache files in `target/<profile>/build/kage-<hash>/out/tauri-codegen-assets/` to verify, or run the binary and look for the marker at runtime.

**Window-show during startup is racy.** The floating, chat, and inline-assist webviews paint at slightly different times after `setup()`. Whichever paints LAST steals focus from any window we just `.show()`d, triggering its `tauri://blur` handler. The floating window's blur handler hides the window; if you programmatically show it during startup (e.g. post-update banner via `maybe_show_floating_after_interactive_install`), use a short-lived suppression flag like `_suppressBlurHideUntil` so the chat window's late paint doesn't dismiss it.

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
- **Inclusive language**: don't use master/slave/whitelist/blacklist; prefer primary/replica, allowlist/denylist, etc.
- **Commits**: never commit unless the user explicitly says "commit". Don't commit on task completion or on "go ahead" / "do it".


## Internationalisation (i18n)

User-visible strings are localised across all three layers — Rust backend,
frontend, and extension sandbox. **The full contract lives in `docs/I18N.md`** —
read that before adding strings or tweaking the i18n machinery. The high-level
rules every contributor needs to know:

- **Logs stay English. Always.** `log::info!`/`warn!`/`error!`/`debug!`,
  `println!`, `eprintln!`, `console.log` all emit English regardless of the
  active locale. Reason: `app.jsonl` from a Japanese user must be searchable
  from any developer's box.
- **Rust user-facing strings** go through `t!("key.path", "name" => value)`
  (macro in `src/i18n.rs`). Errors that bubble to the frontend use
  `AppError::keyed(KIND, "errors.foo.bar", &[("name", &val)])`. The legacy
  free-form constructors (`AppError::connection_lost(s)`, etc.) still work —
  they route through the `errors.passthrough` template — but new code should
  prefer `keyed`.
- **Frontend strings** go through `t("key.path", { name: value })` from
  `ui/js/shared/i18n.js`. Static HTML uses `data-i18n="key.path"` and
  sister attribute variants (`-title`, `-placeholder`, `-aria-label`,
  `-alt`, `-html`); call `applyStaticTranslations(root)` after the markup
  is in the DOM.
- **Catalog**: `locales/<code>/messages.json`. EN is canonical and
  hand-authored; the other 30 are machine-translated by `scripts/translate.py`
  (Claude-powered, per-entry `_source_hash` makes it idempotent). The Rust
  binary embeds every catalog at compile time via `include_str!` in
  `src/i18n.rs::embed_locales!` — adding a language is a one-line change
  there plus a run of `translate.py`.
- **Drift-check is a CI gate**: `scripts/check_i18n.py` (also runs from
  `scripts/test_all.py`). Hard-fails on missing keys in EN, drift between
  EN and any non-EN catalog (in either direction), and `{placeholder}`
  drift. Stale EN keys → warning, not error.
- **AppError translation happens at the serde boundary**, not in domain
  code. `AppError` carries `(kind, key, params)`; its custom `Serialize`
  renders `message` in the active locale at the moment Tauri serialises
  it for the frontend. `Display` (used by logs) uses the EN catalog
  unconditionally.
- **Extension i18n**: each extension ships `_locales/<lang>/messages.json`
  (Chrome convention). The sandbox host loads the matched catalog and an
  EN fallback; the runtime exposes `context.i18n.t(key, vars)` to
  extension code. `__MSG_key__` tokens in the manifest's `name` /
  `description` are resolved before the manifest is shown to the user.
  See `docs/I18N.md` § 6 for the authoring contract; `Kage-Extensions/extensions/hello-world/`
  is the canonical reference.
- **RTL**: when the active language is `ar`/`he`/`fa`/`ur`, the frontend sets
  `<html dir="rtl">` and `body.classList.add('rtl')`. CSS rules in
  `ui/css/shared-rtl.css` flip layout via `[dir=rtl]` selectors. Code,
  URLs, hotkeys, and other inherently-LTR content should carry `dir="ltr"`
  or `class="ltr"`. The existing `ui/js/shared/rtl.js` (per-input direction
  detection) is orthogonal and remains in place.
- **Language preference precedence**: `config.ui.language` (explicit
  override in Settings → Appearance) → `sys-locale` (OS locale) → `en`.
  Region tags strip down to language stems on miss.
- **Settings → Language**: dropdown lives in `ui/js/settings/appearance.js`,
  populated from `get_available_languages`. Selecting a language calls
  `set_language` (Rust) which persists `config.ui.language`, calls
  `i18n::set_language`, and broadcasts `config_updated` so every window
  re-fetches the catalog and re-paints without a restart.

Files engineers touch most often:
- `locales/en/messages.json` — add new keys here first.
- `src/i18n.rs` — Rust loader, `t!` macro.
- `src/error.rs` — `AppError::keyed` is the i18n-native constructor.
- `ui/js/shared/i18n.js` — frontend `t()`, ICU subset, static applier.
- `scripts/check_i18n.py` — drift-check CI gate.
- `scripts/translate.py` — Claude-powered seed/update.
- `docs/I18N.md` — full contract.

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

Settings → Privacy (`ui/js/settings/privacy.js`) lets users toggle and reset their install ID. The welcome screen's privacy step is the initial opt-out surface; `complete_first_run` records their decision via `telemetry::set_consent`. Telemetry is off until a user consents (no `install_id` → `track()` is a no-op), so there's no silent auto-opt-in to migrate away from. (`config_migrations::CURRENT_VERSION` is still `1` and `migrate_one_step` is empty — no migrations have shipped yet.)


## Auto-updates

Signed in-app updates via `tauri-plugin-updater`. See `docs/RELEASE.md` for the full release + signing flow.

**The version is sourced from `Cargo.toml`, never from `tauri.conf.json`.** CI rewrites `Cargo.toml`'s `version` line to a stamped value (`<major>.<minor>.<YYYYMMDDHHMM>+<channel>.<short_sha>`) before building. If `tauri.conf.json` carries a literal `"version"` field, Tauri uses *that* instead of `CARGO_PKG_VERSION` — defeating the stamp. The binary then reports `0.9.0` while `latest.json` advertises a stamped version, and the updater plugin sees them as different forever ("update available" loop). Don't add the field back; if you ever do see it, delete it.

Key points for engineers touching the updater:

- Three channels: `stable`, `beta`, `dev`. Endpoint URLs per channel live in `Cargo.toml [package.metadata.update]`; build.rs exposes them as compile-time env vars; `src/updater.rs::endpoint_for_channel` routes.
- The private signing key lives only in CI (GitHub Actions secret `TAURI_SIGNING_PRIVATE_KEY`). The matching public key lives in `tauri.conf.json → plugins.updater.pubkey` (the single source of truth); `build.rs` reads it from there and re-exports it as the `TAURI_UPDATER_PUBKEY` compile-time env var. Release builds fail loudly if that field is empty.
- The plugin handles: manifest fetch, signature verification, download, and per-platform install + relaunch. `src/updater.rs` wraps it with our scheduling layer (daily check, 5-minute-idle gate for silent installs) and session-resume-after-update (`last-session.txt` handoff via `startup::resolve_resume_session_id`).
- Never call `run_installer` — the plugin owns that now. Deleted from `src/os/mod.rs` in the migration.
- The `Channel` enum in `src/config.rs` is the authority; `#[serde(other)]` on its `Stable` variant means any unknown wire value (a dead channel in a hand-edited config.json) deserializes to `Stable`, so a user can't get trapped on a channel the build doesn't know. `Channel::all()` enumerates them; `get_app_info` exposes the list to the JS dropdown in `ui/js/settings/updates.js`.
- **Windows install mode must be `quiet`** in `tauri.conf.json` → `plugins.updater.windows.installMode`. The default `BasicUi` mode passes only `/UPDATE /ARGS` to the NSIS installer; on Win11 the spawned installer can die silently before its UI surfaces (race between the parent's `process::exit(0)` and the child's window registration). `quiet` produces `/S /R /UPDATE /ARGS` — silent install + auto-relaunch — which avoids the UI race entirely. Verified by the user at the time it was changed.
- **Detach the installer from the Job Object before exit.** `os::install_kill_on_exit_job` adds `KILL_ON_JOB_CLOSE` so our orphan children die with us on crash. ShellExecuteW children inherit the job by default, so the plugin's spawned installer would be reaped along with us. `plugin_download_and_install`'s `on_download_finish` callback runs `graceful_shutdown` → `acp.client.disconnect()` → `os::release_kill_on_exit_job()` (clears the kill flag) before the plugin's `process::exit(0)`. Order matters: explicit child cleanup first (while the job is still safety-netting), THEN release the flag.
- **`fetch_changelog` reads each release's `body` field.** The CI publish action sets `generate_release_notes: true` so GitHub auto-fills the body from commits since the previous release. Without that, dev/beta releases would have empty bodies and the in-app changelog viewer would say "No release notes" even though the GitHub web UI shows commit messages (those come from a separate auto-generated section that isn't in the API's `body`).
- **Install source (`interactive` vs `idle`) is persisted via `install-source.txt`.** Written by both call sites in `updater.rs::persist_install_source` before `plugin_download_and_install`. Consumed at startup by `setup::maybe_show_floating_after_interactive_install` to decide whether to auto-show the floating window post-relaunch (interactive only).

Adding a new channel:
1. Add entry to `[package.metadata.update]` and `[package.metadata.update.dev]` in `Cargo.toml`.
2. Add the corresponding `UPDATE_ENDPOINT_<NAME>` handling to `build.rs`.
3. Add a variant to the `Channel` enum in `src/config.rs` (keep `Stable` last so `#[serde(other)]` stays on it), add it to `Channel::all()`, and add a match arm in `endpoint_for_channel` in `src/updater.rs`.
4. Add a label entry in the `_renderChannelOptions` map in `ui/js/settings/updates.js` (the channel list itself is fetched from Rust via `get_app_info`).
5. Add the CI workflow trigger in `.github/workflows/release.yml`.
6. **Decide migration policy**: should existing users move to the new channel, or stay where they are until they opt in? Adding a new channel almost always means "leave existing users alone" (they didn't ask for it), so usually no migration is needed — and `#[serde(other)]` → `Stable` already keeps anyone on a now-unknown value safe. The exception is if you're *splitting* an existing channel — then you'd add the first real `migrate_one_step` arm (bumping `CURRENT_VERSION`) to redirect users to the appropriate replacement.

Removing the update plugin: the easiest partial rollback is to `option_env!("TAURI_UPDATER_PUBKEY")` → `None`, which makes `plugin_check` a no-op and quietly disables updates. Full removal requires dropping the plugin registration in `main.rs` and the `updater:default` capability — but don't unless you're sure there's no MITM-safe alternative.

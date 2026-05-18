# Kage

A cross-platform desktop AI assistant with a floating window interface. Provides quick access to AI capabilities through a system tray application activated via global shortcuts.

Supported platforms: **Windows 10+** and **macOS 11+** (universal — Apple Silicon and Intel).

## Building

### Prerequisites

All platforms:
- [Rust](https://rustup.rs/) stable toolchain (Edition 2021).
- Tauri CLI v2: `cargo install tauri-cli --version "^2" --locked`.
- Node.js for JS vendor deps + tests: `cd ui/vendor && npm install`.

Windows-specific:
- WebView2 Runtime (ships with Windows 11; installed automatically on Windows 10).
- MSVC build tools (installed with Visual Studio Build Tools or the full IDE).
- [NSIS](https://nsis.sourceforge.io/) only if producing the installer bundle.

macOS-specific:
- Xcode Command Line Tools: `xcode-select --install`. Provides `swiftc` (used to compile the EventKit calendar helper) and `iconutil` (used by the icon generator).
- Optional: [Inkscape](https://inkscape.org/) + Python Pillow if regenerating app icons from `icons/kage-icon-basic.svg`.

### Analytics key (optional)

Kage ships with opt-out [Aptabase](https://aptabase.com) analytics. Builds without a key have the plugin entirely absent — no events, no background worker, no network calls. See [`docs/PRIVACY.md`](docs/PRIVACY.md) for the user-facing policy.

Local builds pick up the key from either (in priority order):

1. `APTABASE_KEY` environment variable
2. A `.aptabase-key` file at the repo root (gitignored)

For your own local release builds, copy the example and paste your key:

```bash
cp .aptabase-key.example .aptabase-key
# edit .aptabase-key and replace the placeholder
```

Debug builds without a key are silent — that's the normal `cargo tauri dev` path for contributors and forks. Release builds without a key emit a `cargo:warning` so you notice if you meant to ship with telemetry and forgot to set it up.

CI reads `APTABASE_KEY` from a GitHub Actions secret of the same name. See `.github/workflows/ci.yml`.

### Release signing key

Kage ships signed in-app updates. Every release artefact is signed with a private key held only in CI; every binary embeds the matching public key at build time so the updater can verify what it downloads. Release builds **fail** if no public key is configured (we never want to ship a binary that can't verify updates).

For local release builds, copy the example and paste your public key:

```bash
cp .tauri-updater-pubkey.example .tauri-updater-pubkey
# edit and paste the contents of .tauri-updater.key.pub
```

Full release + signing documentation lives in [`docs/RELEASE.md`](docs/RELEASE.md).

### Development mode

```bash
cargo tauri dev                  # Run with hot-reloaded frontend
cargo tauri dev -- /dev          # + developer menu, DevTools, tray reload
cargo tauri dev -- /debug        # + ACP protocol message logging
!   # Both
```

The frontend is served from disk via a local dev server, so HTML/CSS/JS edits take effect on reload without a recompile. Only Rust edits require the dev server to rebuild the binary.

### Debug build (binaries only, no installer)

```bash
cargo build                       # Debug profile
cargo build --release             # Release-optimised binaries
```

Debug output:
- `target/debug/kage` (Windows: `kage.exe`)
- `target/debug/kage-computer-control-mcp` (Windows: `.exe`)
- `target/debug/kage-calendar-helper` (macOS only — compiled by `build.rs` via `swiftc`)

Neither `cargo build` nor `cargo build --release` produces a user-installable bundle; they only produce raw binaries.

### Production release build (installer / app bundle)

```bash
cargo tauri build                 # Platform-native installer + bundle
```

Output per platform:
- **Windows**: NSIS installer at `target/release/bundle/nsis/kage_<version>_x64-setup.exe`. MSI is disabled; NSIS is the only distributable.
- **macOS**: `.app` bundle at `target/release/bundle/macos/Kage.app` and DMG at `target/release/bundle/dmg/Kage_<version>_<arch>.dmg`. The calendar helper is bundled inside `Kage.app/Contents/MacOS/` via Tauri's `externalBin` mechanism.

For a universal-binary macOS release covering both Apple Silicon and Intel, either run `cargo tauri build --target universal-apple-darwin` (requires both toolchains installed) or build per-arch and merge with `lipo`.

### Installing unsigned macOS builds

Release builds from CI are not code-signed or notarized (no Apple Developer certificate). macOS Gatekeeper will block them with a "damaged" or "can't be opened" message when downloaded from the internet.

**Before opening the DMG**, strip the quarantine attribute in Terminal:

```bash
xattr -d com.apple.quarantine ~/Downloads/Kage_0.9.0_aarch64.dmg
```

Then open the DMG and drag Kage to Applications as normal.

If you already installed without doing this and get the "corrupted" error:

```bash
# Remove the quarantined install
sudo rm -rf /Applications/Kage.app

# Strip quarantine from the DMG
xattr -d com.apple.quarantine ~/Downloads/Kage_0.9.0_aarch64.dmg

# Re-open the DMG and drag Kage to Applications again
```

> **Why?** On macOS Sequoia (15+), the quarantine and provenance attributes propagate into the `.app` bundle and become immutable — even `sudo xattr -cr` can't remove them after installation. The only reliable fix is to strip quarantine from the DMG *before* mounting it.

### Common commands

```bash
cargo check                       # Fast type/borrow check, no codegen
cargo fmt                         # Auto-format
cargo clippy                      # Lint
cargo test                        # All Rust tests
cd ui/tests && npx vitest run     # JS tests
python scripts/test_all.py        # Rust + JS tests combined
```

## Features

### Floating Window
Summoned via global hotkey (default: Alt+Space on Windows, configurable on macOS). Supports:
- AI chat with streaming responses
- Inline math calculator (type expressions, get instant results)
- Selected text capture from the active window (included as context)
- Application launcher (type app names to launch)
- URL and path detection

### Shortcuts
Custom command shortcuts executed directly from the floating window:
- **Run Program** — launch executables with argument templates
- **Open URL** — open URLs with argument substitution
- **Send Prompt** — send templated messages to the agent
- **Script** — run JavaScript with AI-assisted script generation

Use `{*}` for all arguments, `{0}`, `{1}` for specific ones, `{selection}` for captured text.

For details, see [Shortcuts Guide](docs/SHORTCUTS_GUIDE.md).

### Appearance
- Dark / Light / System theme
- Configurable floating window opacity
- Window start position (center, near mouse, remember last)
- Adjustable font size
- Configurable chat window dimensions

### macOS permissions

Kage requires three macOS privacy permissions, prompted on first use and also surfaceable from **Settings → macOS Permissions**:

- **Accessibility** — UI automation tools and the "paste captured text" flow
- **Input Monitoring** — global hotkey
- **Screen Recording** — reading foreground window titles

These live in System Settings → Privacy & Security. The Welcome wizard walks through each on first launch.

## Frontend Dependencies

JavaScript libraries are managed via npm in `ui/vendor/`:

```bash
cd ui/vendor && npm install
```

After adding a new package, copy its browser bundle to `ui/vendor/lib/` and add a `<script>` tag to the relevant HTML files.

Current dependencies: marked, mermaid, prismjs, @hpcc-js/wasm-graphviz, mathjs

## Documentation

- [Extension Development Guide](docs/EXTENSIONS.md)
- [Security Model](docs/SECURITY_MODEL.md)
- [Debug Mode Guide](docs/DEBUG_MODE.md)
- [Shortcuts Guide](docs/SHORTCUTS_GUIDE.md)
- [OS Abstraction Layer](docs/OS_ABSTRACTION_GUIDE.md)
- [Tool Permissions](docs/TOOL_PERMISSIONS.md)
- [Computer Control MCP: Why It's a Separate Binary](docs/COMPUTER_CONTROL_MCP.md)

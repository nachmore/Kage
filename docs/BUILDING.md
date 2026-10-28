# Building Kage

Everything you need to build, test and release Kage from source. For using Kage, see the [README](../README.md).

## Prerequisites

All platforms:
- [Rust](https://rustup.rs/) stable toolchain (Edition 2021).
- Tauri CLI v2: `cargo install tauri-cli --version "^2" --locked`.
- Node.js for JS vendor deps + tests: `cd ui-vendor && npm install`.

Windows-specific:
- WebView2 Runtime (ships with Windows 11; installed automatically on Windows 10).
- MSVC build tools (installed with Visual Studio Build Tools or the full IDE).
- [NSIS](https://nsis.sourceforge.io/) only if producing the installer bundle.

macOS-specific:
- Xcode Command Line Tools: `xcode-select --install`. Provides the platform linker and `iconutil` (used by the icon generator).
- Optional: [Inkscape](https://inkscape.org/) + Python Pillow if regenerating app icons from `icons/kage-icon-basic.svg`.

## Analytics key (optional)

Kage ships with opt-in [Aptabase](https://aptabase.com) analytics. Builds without a key have the plugin entirely absent — no events, no background worker, no network calls. See [`docs/PRIVACY.md`](PRIVACY.md) for the user-facing policy.

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

## Release signing key

Kage ships signed in-app updates. Every release artefact is signed with a private key held only in CI; every binary embeds the matching public key at build time so the updater can verify what it downloads. Release builds **fail** if no public key is configured (we never want to ship a binary that can't verify updates).

The public key lives in `tauri.conf.json → plugins.updater.pubkey` (committed). Run `./scripts/generate_signing_keys.sh` to generate a keypair and write the public key there automatically.

Full release + signing documentation lives in [`docs/RELEASE.md`](RELEASE.md).

## Development mode

```bash
cargo tauri dev                  # Run with hot-reloaded frontend
cargo tauri dev -- /dev          # + developer menu, DevTools, tray reload
cargo tauri dev -- /debug        # + ACP protocol message logging
cargo tauri dev -- /dev /debug   # Both
```

The frontend is served from disk via a local dev server, so HTML/CSS/JS edits take effect on reload without a recompile. Only Rust edits require the dev server to rebuild the binary.

## Debug build (binaries only, no installer)

```bash
cargo build                       # Debug profile
cargo build --release             # Release-optimised binaries
```

Debug output:
- `target/debug/kage` (Windows: `kage.exe`)
- `target/debug/kage-computer-control-mcp` (Windows: `.exe`)
- `target/debug/kage-calendar-helper` (macOS only)

The sidecars are separate workspace packages, but every build of the main
crate self-provisions them via `build.rs` (built into `target/sidecar-build/`,
staged into `src-tauri/binaries/`, copied next to `kage` by tauri-build) — so
a plain `cargo build` really does produce all the binaries above.

Neither `cargo build` nor `cargo build --release` produces a user-installable bundle; they only produce raw binaries.

## Production release build (installer / app bundle)

```bash
cargo tauri build                 # Platform-native installer + bundle
```

Output per platform:
- **Windows**: NSIS installer at `target/release/bundle/nsis/kage_<version>_x64-setup.exe`. MSI is disabled; NSIS is the only distributable.
- **macOS**: `.app` bundle at `target/release/bundle/macos/Kage.app` and DMG at `target/release/bundle/dmg/Kage_<version>_<arch>.dmg`. The calendar helper is bundled inside `Kage.app/Contents/MacOS/` via Tauri's `externalBin` mechanism.

For a universal-binary macOS release covering both Apple Silicon and Intel, either run `cargo tauri build --target universal-apple-darwin` (requires both toolchains installed) or build per-arch and merge with `lipo`.

## Common commands

```bash
cargo check                       # Fast type/borrow check, no codegen
cargo fmt                         # Auto-format
cargo clippy                      # Lint
cargo test                        # All Rust tests
cd ui-tests && npx vitest run     # JS tests
python scripts/test_all.py        # Rust + JS tests combined
```

## Pre-push hook

`npm install` at the repo root sets up a Husky `pre-push` hook that mirrors CI: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test --lib`, and `npm run ci` (biome). Every `git push` runs them and refuses to push if any fails. Bypass with `git push --no-verify` if you really need to (e.g. a docs-only commit on a clean tree where you trust the gates).

## Frontend dependencies

JavaScript libraries are managed via npm in `ui-vendor/` (outside `ui/` so npm dev tooling doesn't get embedded into the shipped binary):

```bash
cd ui-vendor && npm install
```

The postinstall hook (`ui-vendor/setup.js`) copies the browser bundles into `ui/vendor/lib/`. After adding a new package, extend the COPIES manifest in `setup.js` and add a `<script>` tag pointing at `vendor/lib/...` in the relevant HTML files.

Current dependencies: marked, mermaid, prismjs, @hpcc-js/wasm-graphviz, mathjs

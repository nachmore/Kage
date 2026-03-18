# Technology Stack

## Build System & Language
- Language: Rust (Edition 2021)
- Build tool: Cargo
- Framework: Tauri 2.10 (desktop application framework)

## Core Dependencies
- tauri: Desktop app framework with system tray, global shortcuts, window management
- tokio: Async runtime with full feature set
- serde/serde_json: Serialization and JSON handling
- anyhow: Error handling
- uuid: Unique identifier generation
- dirs: Cross-platform directory paths
- log/env_logger: Logging infrastructure
- chrono: Date and time handling

## Frontend
- Pure HTML/CSS/JavaScript (no framework)
- Located in `ui/` directory
- NPM-managed vendor dependencies in `ui/vendor/` (marked, mermaid, prismjs, mathjs)
- Custom CSS theming with dark/light mode via CSS variables
- ES modules for floating/chat windows, regular scripts for settings

## Common Commands

```bash
# Development (uses Tauri CLI — serves ui/ from a local dev server so
# HTML/JS/CSS changes are picked up on reload without recompiling)
cargo tauri dev -- /dev            # Dev mode (inspector, tray reload)
cargo tauri dev -- /debug          # Debug logging (ACP messages)
cargo tauri dev -- /dev /debug     # Both

# Building
cargo build                # Debug build (binaries only, no installer)
cargo tauri build          # Release build + NSIS installer (output: target/release/bundle/nsis/)

# Note: `cargo build --release` builds optimized binaries but does NOT create
# the installer. Always use `cargo tauri build` for release distribution.
# The computer-control-mcp binary is built automatically alongside the main binary.

# Testing
cargo test                 # All tests
cargo test --test acp_client_test  # Specific test

# Code Quality
cargo check                # Check without building
cargo fmt                  # Format code
cargo clippy               # Lint code

# Frontend Dependencies
cd ui/vendor && npm install  # Install/update JS dependencies
```

### Dev vs Build: Frontend Assets
- `cargo tauri dev` — frontend is served from disk via a local Python HTTP server
  (configured in `tauri.conf.json` → `build.beforeDevCommand` / `build.devUrl`).
  Editing HTML/JS/CSS and using "Reload UX" from the tray picks up changes instantly.
- `cargo tauri build` — frontend is embedded into the binary at compile time and
  the NSIS installer is generated. File changes on disk have no effect until you rebuild.
- `cargo build` / `cargo build --release` — builds raw binaries only (no installer,
  no bundling). Useful for quick iteration but not for distribution.

## Architecture Notes
- Compile-time platform selection via `#[cfg(target_os = "...")]`
- Zero-cost abstractions for cross-platform code
- Win32 API used directly for performance-critical operations (clipboard, SendInput)
- All config fields must have serde defaults for backward compatibility
- Use `log::*` macros for logging, avoid `println!` except for startup banner

## Security
- CSP is intentionally disabled (`"csp": null`) — see `docs/SECURITY_MODEL.md` for rationale
- The security boundary is the tool permission system, not the webview CSP
- Do not add features that load external/untrusted web content without revisiting CSP

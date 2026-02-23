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
# Development
cargo run -- /dev          # Dev mode (inspector, reload)
cargo run -- /debug        # Debug logging (ACP messages)
cargo run -- /dev /debug   # Both

# Building
cargo build                # Debug build
cargo build --release      # Release build (optimized)

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

## Architecture Notes
- Compile-time platform selection via `#[cfg(target_os = "...")]`
- Zero-cost abstractions for cross-platform code
- Win32 API used directly for performance-critical operations (clipboard, SendInput)
- All config fields must have serde defaults for backward compatibility
- Use `log::*` macros for logging, avoid `println!` except for startup banner

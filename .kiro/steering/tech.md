# Technology Stack

## Build System & Language

- Language: Rust (Edition 2021)
- Build tool: Cargo
- Framework: Tauri 1.5 (desktop application framework)

## Core Dependencies

- tauri: Desktop app framework with system tray, global shortcuts, window management
- tokio: Async runtime with full feature set
- serde/serde_json: Serialization and JSON handling
- anyhow: Error handling
- uuid: Unique identifier generation
- dirs: Cross-platform directory paths
- log/env_logger: Logging infrastructure
- chrono: Date and time handling

## Platform-Specific Dependencies

### Windows
- winreg: Windows registry access
- windows-icons: Icon extraction
- windows: Win32 API bindings

### Unix (macOS/Linux)
- libc: C library bindings
- nix: Unix system calls (signals)
- signal-hook: Signal handling

## Frontend

- Pure HTML/CSS/JavaScript (no framework)
- Located in `ui/` directory
- Multiple windows: floating.html, settings.html, index.html
- Custom CSS theming with dark mode support

## Common Commands

### Development
```bash
# Run in development mode (with dev tools)
cargo run -- /dev

# Run with debug logging (ACP protocol messages)
cargo run -- /debug

# Run with both dev and debug modes
cargo run -- /dev /debug
```

### Building
```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release
```

### Testing
```bash
# Run all tests
cargo test

# Run specific test file
cargo test --test acp_client_test

# Run with output
cargo test -- --nocapture
```

### Code Quality
```bash
# Check code without building
cargo check

# Format code
cargo fmt

# Lint code
cargo clippy
```

## Build Configuration

Release builds use aggressive optimization:
- opt-level = 3 (maximum optimization)
- lto = true (link-time optimization)
- codegen-units = 1 (better optimization, slower compile)

## Architecture Notes

- Uses compile-time platform selection via `#[cfg(target_os = "...")]`
- Zero-cost abstractions for cross-platform code
- No dynamic dispatch or runtime overhead for OS abstraction layer

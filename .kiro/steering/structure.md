# Project Structure

## Directory Organization

```
kiro-assistant/
├── src/                    # Rust source code
│   ├── main.rs            # Application entry point, Tauri setup, hotkey registration
│   ├── lib.rs             # Library root, module declarations
│   ├── state.rs           # AppState struct shared across commands
│   ├── tray.rs            # System tray icon and menu setup
│   ├── acp_client.rs      # Agent Communication Protocol client
│   ├── app_launcher.rs    # Application discovery and launching
│   ├── config.rs          # Configuration management
│   ├── logger.rs          # File-based logging system
│   ├── process_manager.rs # Process lifecycle management
│   ├── commands/          # Tauri command handlers (IPC from frontend)
│   │   ├── mod.rs         # Re-exports all command modules
│   │   ├── window.rs      # Window management (toggle, resize, position, context menu)
│   │   ├── messaging.rs   # ACP messaging (streaming, permissions, connection)
│   │   ├── input.rs       # Input routing (URL/path detection, app launch, shortcuts)
│   │   └── system.rs      # System commands (config, clipboard, devtools, quit)
│   └── os/                # OS abstraction layer
│       ├── mod.rs         # Platform selection and re-exports
│       ├── cursor.rs      # Cross-platform cursor API
│       ├── launcher.rs    # Cross-platform app launcher API
│       ├── process.rs     # Cross-platform process API
│       ├── shell.rs       # Cross-platform shell operations
│       ├── windows/       # Windows-specific implementations
│       ├── macos/         # macOS-specific implementations
│       └── linux/         # Linux-specific implementations
├── tests/                 # Integration tests
├── ui/                    # Frontend assets
│   ├── *.html            # Window HTML files
│   ├── css/              # Stylesheets
│   ├── js/               # JavaScript modules
│   └── assets/           # Images and icons
├── docs/                  # Documentation
├── icons/                 # Application icons
└── .kiro/                # Kiro configuration
    ├── specs/            # Feature specifications
    └── steering/         # AI assistant guidance
```

## Key Modules

### Core Application (`src/`)

- `main.rs`: Tauri app initialization, hotkey registration, plugin setup (~240 lines)
- `state.rs`: `AppState` struct shared across all command handlers
- `tray.rs`: System tray icon, menu construction, and menu event handlers
- `acp_client.rs`: Handles communication with kiro-cli backend service
- `app_launcher.rs`: Scans system for installed applications, manages app registry
- `config.rs`: Loads/saves user configuration (hotkeys, shortcuts, permissions)
- `logger.rs`: File-based logging with rotation
- `process_manager.rs`: Tracks and manages spawned child processes

### Tauri Commands (`src/commands/`)

All `#[tauri::command]` handlers are organized by domain:

- `window.rs`: Window management (toggle, resize, drag, settings, context menu)
- `messaging.rs`: ACP messaging (streaming chat, permissions, connection checks)
- `input.rs`: Input routing (URL/path detection, app launching, shortcut execution)
- `system.rs`: System utilities (config get/save, tool permissions, clipboard, devtools, quit)

### OS Abstraction Layer (`src/os/`)

The OS abstraction layer provides a clean, platform-agnostic API. All platform-specific code is isolated in subdirectories.

**Cross-platform APIs** (in `src/os/*.rs`):
- Use these functions from application code
- They dispatch to platform-specific implementations at compile time

**Platform implementations** (in `src/os/{windows,macos,linux}/`):
- Each platform directory contains matching modules
- Functions end with `_impl` suffix
- Never call these directly from application code

### Frontend (`ui/`)

- `floating.html`: Main floating window interface
- `settings.html`: Settings and configuration UI
- `index.html`: Main window (currently hidden by default)
- `js/settings/`: Modular settings management system
- `css/floating-*.css`: Theming and component styles

### Tests (`tests/`)

Integration tests for core functionality:
- `acp_client_test.rs`: ACP protocol tests
- `app_launcher_test.rs`: Application scanning tests
- `config_test.rs`: Configuration loading/saving tests
- `mock_acp_server.rs`: Mock server for testing

## Architecture Patterns

### OS Abstraction Pattern

When adding platform-specific functionality:

1. Define cross-platform API in `src/os/module.rs`
2. Implement `module_impl()` functions in each platform directory
3. Use `#[cfg(target_os = "...")]` for compile-time dispatch
4. Export from `src/os/mod.rs`

Example:
```rust
// Application code
use crate::os;
os::get_cursor_position()

// Dispatches to:
// - src/os/windows/cursor.rs on Windows
// - src/os/macos/cursor.rs on macOS
// - src/os/linux/cursor.rs on Linux
```

### Configuration Pattern

- Config files stored in user's config directory (via `dirs` crate)
- JSON format with serde serialization
- Loaded at startup, saved on changes
- Validated with defaults for missing fields

### Window Management Pattern

- Multiple Tauri windows defined in `tauri.conf.json`
- Windows created at startup but hidden
- Show/hide via IPC commands from frontend
- Each window has dedicated HTML/JS/CSS

## File Naming Conventions

- Rust files: `snake_case.rs`
- Test files: `*_test.rs` in `tests/` directory
- HTML files: `kebab-case.html`
- CSS files: `kebab-case-purpose.css`
- JS files: `kebab-case-module.js`

## Import Conventions

- Use `crate::` for internal imports
- Use `use crate::os;` for OS abstraction layer
- Avoid `use crate::os::windows::*` (never import platform modules directly)
- Group imports: std, external crates, internal modules

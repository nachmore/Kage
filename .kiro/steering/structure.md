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
│   ├── config.rs          # Configuration management (hotkeys, shortcuts, UI, math, permissions)
│   ├── logger.rs          # File-based logging system
│   ├── process_manager.rs # Process lifecycle management
│   ├── auto_steering.rs   # Auto-generated steering document from conversation context
│   ├── builtin_steering.md # Built-in steering content sent to agent
│   ├── commands/          # Tauri command handlers (IPC from frontend)
│   │   ├── mod.rs         # Re-exports all command modules
│   │   ├── window.rs      # Window management, positioning, selection capture, context menu
│   │   ├── messaging.rs   # ACP messaging (streaming, permissions, connection)
│   │   ├── input.rs       # Input routing (URL/path detection, app launch, shortcuts)
│   │   ├── sessions.rs    # Session management (list, load, switch, rename)
│   │   └── system.rs      # System commands (config, clipboard, devtools, quit, steering)
│   └── os/                # OS abstraction layer
│       ├── mod.rs         # Platform selection and re-exports
│       ├── cursor.rs      # Cross-platform cursor API
│       ├── launcher.rs    # Cross-platform app launcher API
│       ├── process.rs     # Cross-platform process API
│       ├── shell.rs       # Cross-platform shell operations
│       ├── user.rs        # Cross-platform user info
│       ├── windows/       # Windows-specific implementations
│       ├── macos/         # macOS-specific implementations
│       └── linux/         # Linux-specific implementations
├── tests/                 # Integration tests
├── ui/                    # Frontend assets
│   ├── *.html            # Window HTML files (floating, index, settings, context-menu)
│   ├── css/              # Stylesheets (shared tokens, components, themes)
│   ├── js/               # JavaScript modules
│   │   ├── floating-*.js # Floating window modules (app, commands, context-menu, main, markdown, permissions, suggestions, theme, window)
│   │   ├── chat-*.js     # Chat window modules (app, main, permissions)
│   │   ├── tool-utils.js # Shared tool icon/emoji/escapeHtml utilities
│   │   ├── math-eval.js  # Math expression evaluator (wraps mathjs)
│   │   ├── attachments.js # Image paste/drag-drop handling
│   │   └── settings/     # Settings modules (one per section)
│   ├── vendor/           # NPM-managed JS dependencies (marked, mermaid, prismjs, mathjs)
│   └── assets/           # Images and icons
├── docs/                  # Documentation
├── icons/                 # Application icons
└── .kiro/                # Kiro configuration
    ├── specs/            # Feature specifications
    └── steering/         # AI assistant guidance
```

## Key Patterns

### Shared Utilities
- `ui/js/tool-utils.js` — shared `getToolIcon()`, `getToolEmoji()`, `escapeHtml()` used across floating, chat, settings, and permissions UIs. Always import from here, never duplicate.

### OS Abstraction
- Define cross-platform API in `src/os/module.rs`
- Implement `module_impl()` in each platform directory
- Use `#[cfg(target_os = "...")]` for compile-time dispatch
- Never import platform modules directly from application code

### Configuration
- JSON format with serde, stored in user's config directory
- All fields must have `#[serde(default)]` for backward compatibility
- Changes propagated via `config_updated` Tauri event

### Theme System
- CSS variables in `shared-kiro-tokens.css` (dark defaults, light overrides via `body.light-theme`)
- Theme applied via `loadAndApplyTheme()` in `floating-theme.js`
- All windows listen for `config_updated` to reapply theme

### Frontend Dependencies
- Managed via npm in `ui/vendor/`, browser bundles in `ui/vendor/lib/`
- Loaded via `<script>` tags, not ES module imports

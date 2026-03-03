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
│   │   ├── shared/       # Shared modules (used by both floating + chat windows)
│   │   │   ├── markdown.js, theme.js, commands.js, speech.js, shortcuts.js
│   │   │   ├── attachments.js, streaming-utils.js, tool-utils.js, notify.js
│   │   │   ├── extension-manager.js, math-eval.js, timer-sounds.js, hotkey-picker.js
│   │   ├── floating/     # Floating window modules (app, main, window, suggestions, search, etc.)
│   │   ├── chat/         # Chat window modules (app, main, permissions)
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
- `ui/js/shared/tool-utils.js` — shared `getToolIcon()`, `getToolEmoji()`, `escapeHtml()` used across floating, chat, settings, and permissions UIs. Always import from here, never duplicate.
- `ui/css/shared-components.css` — shared component styles (keycaps, hotkey picker, etc.) used across all windows. Add reusable styles here, never duplicate into window-specific CSS files. Must be loaded in every window's HTML.
- `ui/css/shared-kiro-tokens.css` — CSS variables (colors, spacing). Loaded in every window.
- Never duplicate styles or code across files. If something is used in more than one window, move it to a shared file.

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

### Settings Window
- Each settings section is a JS class extending `SettingsModule` (defined in `base.js`)
- Modules are registered in `manager.js` in sidebar order, rendered into `#settingsModules`
- First registered module is visible by default; others are hidden via `.hidden` class
- Sidebar items use `data-section` attribute matching the module `id`
- All modules must implement: `render()`, `load(config)`, `save(config)`, `validate()`
- Optional: `initialize()` (called after render), `destroy()` (cleanup)
- Use `createCheckboxRow()`, `createControlRow()` helpers from base class for consistent layout
- CAVEAT: When rendering markdown with `marked.parse()`, always sanitize the input first. If the source returns HTML instead of markdown, marked will pass it through raw — injecting `<style>` and `<script>` tags that corrupt the page. Check for HTML document markers (`<!`, `<html`) and wrap in a code fence before parsing.

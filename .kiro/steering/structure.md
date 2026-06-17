# Project Structure

## Directory Organization

```
kage/
├── src/                    # Rust source code
│   ├── main.rs            # Application entry point, Tauri setup, hotkey registration
│   ├── lib.rs             # Library root, module declarations
│   ├── state.rs           # AppState struct shared across commands
│   ├── tray.rs            # System tray icon and menu setup
│   ├── acp_client/        # Agent Communication Protocol client (mod, session, transport, types)
│   ├── app_launcher.rs    # Application discovery and launching
│   ├── config.rs          # Configuration management (hotkeys, shortcuts, UI, math, permissions)
│   ├── error.rs           # Error types
│   ├── extensions.rs      # Extension/theme discovery and installation
│   ├── logger.rs          # File-based logging system
│   ├── mcp_registration.rs # MCP server registration in mcp.json
│   ├── process_manager.rs # Process lifecycle management with orphaned process cleanup
│   ├── single_instance.rs # Single-instance enforcement with OS-level file locking
│   ├── updater.rs         # Auto-update system with idle detection and silent updates
│   ├── auto_steering.rs   # Auto-generated steering document from conversation context
│   ├── builtin_steering.md # Built-in steering content sent to agent
│   ├── computer_control/  # Computer control MCP server (mod, tree, app_steering/)
│   ├── commands/          # Tauri command handlers (IPC from frontend)
│   │   ├── mod.rs         # Re-exports all command modules
│   │   ├── window.rs      # Window management, positioning, selection capture, context menu
│   │   ├── messaging.rs   # ACP messaging (streaming, permissions, connection)
│   │   ├── input.rs       # Input routing (URL/path detection, app launch, shortcuts)
│   │   ├── sessions.rs    # Session management (list, load, switch, rename)
│   │   ├── system.rs      # System commands (config, clipboard, devtools, quit, steering)
│   │   ├── extensions.rs  # Extension management commands
│   │   ├── folder_tools.rs # Folder/file operations (scan, plan, execute — also used by kage-computer-control-mcp)
│   │   ├── pocket_tts.rs  # Pocket TTS server management
│   │   └── kiro_desktop.rs # Kiro IDE session viewer commands
│   └── os/                # OS abstraction layer
│       ├── mod.rs         # Platform selection and re-exports
│       ├── cursor.rs      # Cross-platform cursor API
│       ├── launcher.rs    # Cross-platform app launcher API
│       ├── process.rs     # Cross-platform process API
│       ├── shell.rs       # Cross-platform shell operations
│       ├── user.rs        # Cross-platform user info
│       ├── clipboard.rs   # Cross-platform clipboard (read, write, selection capture)
│       ├── clipboard_history.rs # Clipboard history API
│       ├── file_search.rs # Cross-platform file search (Windows Everything, macOS mdfind)
│       ├── calendar.rs    # Calendar event integration
│       ├── startup.rs     # Launch-on-startup management
│       ├── hotkey.rs      # Hotkey capture API
│       ├── icon.rs        # Application icon extraction
│       ├── window_list.rs # Window listing and focus
│       ├── accessibility.rs # Accessibility/UI automation API
│       ├── windows/       # Windows-specific implementations (full)
│       ├── macos/         # macOS-specific implementations (full)
│       └── linux/         # Linux-specific implementations (partial)
├── tests/                 # Integration tests
├── ui/                    # Frontend assets
│   ├── floating.html      # Main floating window
│   ├── index.html         # Chat window
│   ├── settings.html      # Settings UI
│   ├── context-menu.html  # Context menu
│   ├── store.html         # Extension store
│   ├── welcome.html       # First-run experience
│   ├── css/              # Stylesheets (shared tokens, components, themes)
│   ├── js/               # JavaScript modules
│   │   ├── shared/       # Shared modules (used by both floating + chat windows)
│   │   │   ├── markdown.js, theme.js, commands.js, speech.js, shortcuts.js
│   │   │   ├── attachments.js, streaming-utils.js, tool-utils.js, notify.js
│   │   │   ├── extension-manager.js, timer-sounds.js, hotkey-picker.js
│   │   │   ├── link-handler.js, quick-actions.js, result-executor.js
│   │   │   └── rtl.js, search-engine.js, tts-streamer.js, network.js
│   │   ├── floating/     # Floating window modules
│   │   │   ├── app.js, main.js, window.js, suggestions.js, search-unified.js
│   │   │   ├── permissions.js, clipboard-history.js, color.js
│   │   │   └── context-menu.js, devtools.js, timer.js
│   │   ├── chat/         # Chat window modules (app, main, permissions, agent-sessions)
│   │   └── settings/     # Settings modules (base, manager, + one per section)
│   ├── vendor/           # NPM-managed JS dependencies (marked, mermaid, prismjs, mathjs, graphviz)
│   │                     # Run `npm install` to populate lib/ (not checked into git)
│   ├── assets/           # Images and icons
│   ├── extensions/       # Built-in extensions (math, calendar, window-walker)
│   ├── themes/           # Custom themes
│   └── updates/          # Update staging area
├── store/                 # Extension store
│   ├── packages/         # Zipped extension/theme packages for store download
│   ├── extensions/       # Store extension source (todos, color-picker, dev-tools, timer, dictionary, focus-tracker, hello-world, link-preview)
│   └── themes/           # Store theme source (nord, sunset)
├── docs/                  # Documentation
├── icons/                 # Application icons
├── scripts/              # Development and utility scripts (Python, PowerShell)
├── sample_sessions/      # Sample ACP session data for testing
├── gen/                  # Generated schemas (ACL manifests, capabilities, desktop/windows schemas)
├── capabilities/         # Default capability definitions
└── .kiro/                # Kiro IDE configuration (not app-related)
    ├── specs/            # Feature specifications
    └── steering/         # AI assistant guidance
```

## Key Patterns

### Shared Utilities
- `ui/js/shared/tool-utils.js` — shared `getToolIcon()`, `getToolEmoji()`, `escapeHtml()` used across floating, chat, settings, and permissions UIs. Always import from here, never duplicate.
- `ui/css/shared-components.css` — shared component styles (keycaps, hotkey picker, etc.) used across all windows. Add reusable styles here, never duplicate into window-specific CSS files. Must be loaded in every window's HTML.
- `ui/css/shared-kage-tokens.css` — CSS variables (colors, spacing). Loaded in every window.
- Never duplicate styles or code across files. If something is used in more than one window, move it to a shared file.

### OS Abstraction
- Define cross-platform API in `src/os/module.rs`
- Implement `module_impl()` in each platform directory
- Use `#[cfg(target_os = "...")]` for compile-time dispatch
- Never import platform modules directly from application code
- Windows: fully implemented. macOS: fully implemented (accessibility, calendar via EventKit, hotkey, icon, cursor, power, file search, window list, clipboard paste, TCC onboarding). Linux: partial — relies on stubs for AX-equivalent surface.

### Configuration
- JSON format with serde, stored in user's config directory
- All fields must have `#[serde(default)]` for backward compatibility
- Changes propagated via `config_updated` Tauri event

### Theme System
- CSS variables in `shared-kage-tokens.css` (dark defaults, light overrides via `body.light-theme`)
- Theme applied via `loadAndApplyTheme()` in `theme.js`
- All windows listen for `config_updated` to reapply theme

### Frontend Dependencies
- Managed via npm in `ui-vendor/` (outside `ui/`), browser bundles copied to `ui/vendor/lib/` by `setup.js`
- `lib/` is gitignored — regenerated automatically on first `cargo tauri dev` or `cargo tauri build`
- Loaded via `<script>` tags, not ES module imports

### Settings Window
- Each settings section is a JS class extending `SettingsModule` (defined in `base.js`)
- Modules are registered in `manager.js` in sidebar order, rendered into `#settingsModules`
- First registered module is visible by default; others are hidden via `.hidden` class
- Sidebar items use `data-section` attribute matching the module `id`
- All modules must implement: `render()`, `load(config)`, `save(config)`, `validate()`
- Optional: `initialize()` (called after render), `destroy()` (cleanup)
- Use `createCheckboxRow()`, `createControlRow()` helpers from base class for consistent layout
- Current modules: about, appearance, assistant, connection, hotkey, integration, math, mcp, model, notifications, shortcuts, speech, store, system, tool-permissions, updates
- CAVEAT: When rendering markdown with `marked.parse()`, always sanitize the input first. If the source returns HTML instead of markdown, marked will pass it through raw — injecting `<style>` and `<script>` tags that corrupt the page. Check for HTML document markers (`<!`, `<html`) and wrap in a code fence before parsing.

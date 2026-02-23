# Kiro Assistant

A cross-platform desktop AI assistant with a floating window interface. Provides quick access to AI capabilities through a system tray application activated via global shortcuts.

## Quick Start

```bash
cargo run -- /dev          # Development mode (inspector + reload)
cargo run -- /debug        # Debug logging (ACP protocol messages)
cargo build --release      # Optimized release build
```

## Features

### Floating Window
Summoned via global hotkey (default: Alt+Space). Supports:
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

## Frontend Dependencies

JavaScript libraries are managed via npm in `ui/vendor/`:

```bash
cd ui/vendor && npm install
```

After adding a new package, copy its browser bundle to `ui/vendor/lib/` and add a `<script>` tag to the relevant HTML files.

Current dependencies: marked, mermaid, prismjs, @hpcc-js/wasm-graphviz, mathjs

## Documentation

- [Debug Mode Guide](docs/DEBUG_MODE.md)
- [Shortcuts Guide](docs/SHORTCUTS_GUIDE.md)
- [OS Abstraction Guide](docs/OS_ABSTRACTION_GUIDE.md)
- [OS Architecture](docs/OS_ARCHITECTURE.md)
- [Tool Permissions](docs/TOOL_PERMISSIONS.md)

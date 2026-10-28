# Kage

A cross-platform desktop AI assistant with a floating window interface. Provides quick access to AI capabilities through a system tray application activated via global shortcuts.

Supported platforms: **Windows 10+** and **macOS 11+** (universal — Apple Silicon and Intel).

## Install

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

## Building from source

```bash
cargo install tauri-cli --version "^2" --locked
cd ui-vendor && npm install && cd ..
cargo tauri dev
```

Prerequisites, release builds, signing keys, analytics keys and the test suite are covered in **[docs/BUILDING.md](docs/BUILDING.md)**.

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

## Documentation

- [Building Kage](docs/BUILDING.md)
- [Extension Development Guide](docs/EXTENSIONS.md)
- [Security Model](docs/SECURITY_MODEL.md)
- [Debug Mode Guide](docs/DEBUG_MODE.md)
- [Shortcuts Guide](docs/SHORTCUTS_GUIDE.md)
- [OS Abstraction Layer](docs/OS_ABSTRACTION_GUIDE.md)
- [Tool Permissions](docs/TOOL_PERMISSIONS.md)
- [Computer Control MCP: Why It's a Separate Binary](docs/COMPUTER_CONTROL_MCP.md)

# Product Overview

Kiro Assistant is a cross-platform desktop AI assistant with a floating window interface. It provides quick access to AI capabilities through a system tray application activated via global shortcuts.

## Core Features

- Floating window interface for quick AI interactions
- System tray integration for background operation
- Custom command shortcuts (run programs, open URLs, send prompts, run scripts)
- Script shortcuts with JS execution and AI-assisted script generation
- Inline math calculator (evaluates expressions as you type)
- Selected text capture from active window (included as context for prompts)
- Agent Communication Protocol (ACP) client for connecting to kiro-cli backend
- Cross-platform support (Windows fully implemented, macOS/Linux partial)
- Dark/Light/System theme with configurable opacity and font size
- Configurable window positioning (center, near mouse, remember last position)
- Tool permission management with auto-associated emojis
- Settings management UI with modular section architecture
- Extension system with theme and command pack support (store UI)
- Auto-update system with idle detection and silent updates
- Single-instance enforcement
- MCP server registration and management
- Clipboard history (Windows)
- File search integration (Windows Everything API)
- Calendar event integration (Windows Outlook)
- Pocket TTS (text-to-speech) server integration
- Computer control MCP server (accessibility tree, UI automation)
- Folder organization tools via MCP (scan, plan, execute, native folder picker)
- Network connectivity detection with offline mode (local features still work)
- RTL language support
- Welcome/first-run experience

## Key Components

- Floating window: Always-on-top, transparent, minimal UI for quick queries
- Settings window: Configuration interface with sidebar navigation (20+ modules)
- Chat window: Expanded conversation view with session management
- Extension store: Browse and install extensions/themes
- Welcome window: First-run onboarding experience
- System tray: Background presence with quick access menu
- ACP client: Handles communication with the AI backend service (session, transport, types)
- Process manager: Manages spawned processes and child applications
- Application launcher: Discovers and launches system applications
- Updater: Auto-update with version checking and installer download
- Computer control MCP: Accessibility tree inspection, UI automation, and folder organization tools

# Quick Reference Card

## Configuration

### Spawn Command Format:
```
<full-path-to-binary> <arguments>
```

### Examples:

**Windows:**
```
C:\workplace\kiro-cli\target\release\chat_cli.exe acp
```

**Linux:**
```
/home/user/kiro-cli/target/release/chat_cli acp
```

**macOS:**
```
/Users/user/kiro-cli/target/release/chat_cli acp
```

## Configuration File Location

| Platform | Path |
|----------|------|
| Windows | `%APPDATA%\kiro-assistant\config.json` |
| Linux | `~/.config/kiro-assistant/config.json` |
| macOS | `~/Library/Application Support/kiro-assistant/config.json` |

## Logs Location

| Platform | Path |
|----------|------|
| Windows | `%APPDATA%\kiro-assistant\logs\` |
| Linux | `~/.config/kiro-assistant/logs\` |
| macOS | `~/Library/Application Support/kiro-assistant/logs\` |

## Key Features

| Feature | Status |
|---------|--------|
| Hidden process spawning | ✅ |
| Real-time streaming | ✅ |
| Cross-platform | ✅ |
| Error handling | ✅ |
| Auto-termination | ✅ |

## Hotkeys

| Action | Default Hotkey |
|--------|----------------|
| Toggle floating window | Alt+Space |
| Hide window | Escape |
| Send message | Enter |
| Navigate suggestions | Arrow Up/Down |

## Troubleshooting

| Issue | Solution |
|-------|----------|
| Console window appears | Use release build, check CREATE_NO_WINDOW flag |
| Can't connect | Verify spawn command path, check logs |
| No response | Check ACP server is running, view logs |
| Process won't die | Manually kill: `taskkill /F /IM chat_cli.exe` |

## Build Commands

```bash
# Build Kiro Assistant
cargo build --release

# Build ACP Server
cd kiro-cli
cargo build --release

# Run Kiro Assistant
./target/release/kiro-assistant
```

## Configuration Example

```json
{
  "version": 1,
  "acp": {
    "spawn_command": "C:\\path\\to\\chat_cli.exe acp",
    "host": "127.0.0.1",
    "port": 8765,
    "timeout_ms": 30000
  }
}
```

## Event Flow

```
User Input → send_message_streaming
  ↓
message_chunk (streaming)
  ↓
message_complete (done)
```

## Common Commands

```bash
# Check if process is running (Windows)
tasklist | findstr chat_cli

# Check if process is running (Linux/macOS)
ps aux | grep chat_cli

# Kill process (Windows)
taskkill /F /IM chat_cli.exe

# Kill process (Linux/macOS)
pkill chat_cli
```

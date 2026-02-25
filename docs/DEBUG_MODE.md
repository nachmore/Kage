# Debug Mode

Debug mode provides detailed logging of ACP (Agent Communication Protocol) messages to the console for troubleshooting and development purposes.

## Enabling Debug Mode

Start the application with the `/debug` or `--debug` command-line argument:

```bash
kiro-assistant.exe /debug
```

or

```bash
kiro-assistant.exe --debug
```

When enabled, you'll see a startup message:
```
🐛 DEBUG MODE ENABLED - Detailed ACP logs will be printed to console
```

## What Gets Logged

When debug mode is enabled, all logs are printed to the console (stdout) in addition to the log file:

### 1. ACP Request Messages
- Full JSON payload of every request sent to the ACP server
- Request method and ID
- All parameters included in the request

### 2. ACP Response Messages  
- Full JSON payload of every response received
- Response type (TCP or Pipe connection)
- Raw response data

### 3. Chat Messages
- Complete message content being sent
- Message length
- Full request structure for session/prompt calls

### 4. Streaming Updates
- Each notification received during streaming
- Accumulated response chunks
- Session update events

## Console Output Format

Debug logs are printed with timestamps:
```
[10:30:45.123] INFO 🐛 [ACP DEBUG] ═══════════════════════════════════════
[10:30:45.124] INFO 🐛 [ACP DEBUG] 📤 SENDING REQUEST
[10:30:45.124] INFO 🐛 [ACP DEBUG] Method: session/prompt
[10:30:45.124] INFO 🐛 [ACP DEBUG] ID: 2
[10:30:45.125] INFO 🐛 [ACP DEBUG] Full JSON: {"jsonrpc":"2.0",...}
[10:30:45.125] INFO 🐛 [ACP DEBUG] ═══════════════════════════════════════
```

## Log File Location

Logs are also written to a file regardless of debug mode. The location varies by platform:

- **Windows**: `%LOCALAPPDATA%\kiro-assistant\logs\kiro-assistant.log`
- **macOS**: `~/Library/Application Support/kiro-assistant/logs/kiro-assistant.log`
- **Linux**: `~/.local/share/kiro-assistant/logs/kiro-assistant.log`

## Use Cases

- Troubleshooting connection issues with kiro-cli
- Debugging message format problems
- Understanding the ACP protocol flow
- Development and testing of new features
- Investigating response parsing errors
- Real-time monitoring of ACP communication

## Combining with Dev Mode

You can combine debug mode with dev mode for full development capabilities:

```bash
cargo tauri dev -- /debug /dev
```

This enables:
- Debug logging to console
- Developer menu items in system tray
- DevTools access
- UI reload functionality

## Performance Impact

Debug mode adds minimal overhead as it only affects logging output. Console output is synchronous, so there may be a slight performance impact during heavy ACP communication, but it's negligible for normal usage.

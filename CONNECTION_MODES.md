# ACP Connection Modes

## Overview

The Kiro Assistant now supports two distinct connection modes for the ACP server:

1. **Local Mode** - Spawns and manages the ACP process locally (no TCP connection)
2. **Remote Mode** - Connects to an external ACP server via TCP

Only one mode can be active at a time.

## Local Mode

### How It Works:
- Assistant spawns the ACP process using the specified command
- Process runs hidden in the background (no console window)
- **No TCP connection is established**
- Process communicates directly with the assistant
- Process is automatically terminated when assistant closes

### Configuration:
```json
{
  "acp": {
    "mode": {
      "type": "local",
      "spawn_command": "C:\\workplace\\kiro-cli\\target\\release\\chat_cli.exe acp"
    }
  }
}
```

### Use Cases:
- Development with local builds
- Single-user desktop usage
- No network configuration needed
- Maximum security (no network exposure)

### Advantages:
- ✅ No TCP connection overhead
- ✅ Automatic process management
- ✅ No port conflicts
- ✅ No firewall issues
- ✅ More secure (no network exposure)

## Remote Mode

### How It Works:
- Assistant connects to an existing ACP server via TCP
- Server must be started manually or by another process
- Supports remote servers on different machines
- Connection retries with exponential backoff

### Configuration:
```json
{
  "acp": {
    "mode": {
      "type": "remote",
      "host": "127.0.0.1",
      "port": 8765,
      "timeout_ms": 30000
    }
  }
}
```

### Use Cases:
- Connecting to remote ACP servers
- Multiple clients connecting to same server
- Server running as a service
- Development/testing with external servers

### Advantages:
- ✅ Can connect to remote servers
- ✅ Multiple clients can share one server
- ✅ Server can run independently
- ✅ Useful for distributed setups

## Configuration via Settings UI

### Switching Modes:

1. Open Settings (system tray → Settings)
2. Go to "Connection" section
3. Select mode from dropdown:
   - **Local (Spawn Process)** - For local development
   - **Remote (TCP Connection)** - For remote servers

### Local Mode Settings:
- **Spawn Command**: Full command to start ACP server
  - Example: `C:\path\to\chat_cli.exe acp`
  - Must include binary path and arguments

### Remote Mode Settings:
- **Host**: Server hostname or IP address
- **Port**: Server port number
- **Timeout**: Connection timeout in milliseconds

## Migration from Old Configuration

### Old Format (Deprecated):
```json
{
  "acp": {
    "host": "127.0.0.1",
    "port": 8765,
    "timeout_ms": 30000,
    "spawn_command": "C:\\path\\to\\chat_cli.exe acp"
  }
}
```

### New Format (Local Mode):
```json
{
  "acp": {
    "mode": {
      "type": "local",
      "spawn_command": "C:\\path\\to\\chat_cli.exe acp"
    }
  }
}
```

### New Format (Remote Mode):
```json
{
  "acp": {
    "mode": {
      "type": "remote",
      "host": "127.0.0.1",
      "port": 8765,
      "timeout_ms": 30000
    }
  }
}
```

## Choosing the Right Mode

### Use Local Mode When:
- Running on a single machine
- Developing locally
- Want automatic process management
- Don't need network access
- Want maximum security

### Use Remote Mode When:
- Connecting to a remote server
- Multiple clients need to share a server
- Server runs as a system service
- Need to connect across network
- Testing distributed setups

## Technical Details

### Local Mode Implementation:
- Spawns process with hidden window (Windows: CREATE_NO_WINDOW)
- Redirects stdio to null
- Process detachment on Unix (setsid)
- No TCP connection established
- Direct IPC communication

### Remote Mode Implementation:
- TCP connection with retry logic
- Exponential backoff (100ms → 30s)
- Connection timeout configurable
- Automatic reconnection attempts
- Standard JSON-RPC over TCP

## Troubleshooting

### Local Mode Issues:

**"Failed to spawn Kiro process"**
- Verify spawn command is correct
- Check binary exists at specified path
- Ensure binary has execute permissions (Unix)
- Check logs for detailed error

**Process appears in Task Manager**
- This is normal - process runs hidden
- No console window should be visible
- Process will terminate with assistant

### Remote Mode Issues:

**"Connection refused"**
- Verify server is running
- Check host and port are correct
- Ensure no firewall blocking connection
- Verify server is listening on specified port

**"Connection timeout"**
- Increase timeout_ms value
- Check network connectivity
- Verify server is responsive

## Examples

### Local Mode (Windows):
```json
{
  "acp": {
    "mode": {
      "type": "local",
      "spawn_command": "C:\\Program Files\\Kiro\\chat_cli.exe acp"
    }
  }
}
```

### Local Mode (Linux/macOS):
```json
{
  "acp": {
    "mode": {
      "type": "local",
      "spawn_command": "/usr/local/bin/chat_cli acp"
    }
  }
}
```

### Remote Mode (Localhost):
```json
{
  "acp": {
    "mode": {
      "type": "remote",
      "host": "127.0.0.1",
      "port": 8765,
      "timeout_ms": 30000
    }
  }
}
```

### Remote Mode (Remote Server):
```json
{
  "acp": {
    "mode": {
      "type": "remote",
      "host": "acp-server.example.com",
      "port": 8765,
      "timeout_ms": 60000
    }
  }
}
```

## Summary

The new mode system provides clear separation between local and remote connections:
- **Local Mode**: Spawn and manage process locally, no TCP
- **Remote Mode**: Connect to external server via TCP

Choose the mode that best fits your use case!

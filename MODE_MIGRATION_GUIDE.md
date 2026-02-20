# Migration Guide: Connection Modes

## What Changed?

The ACP configuration has been refactored into two distinct modes:
- **Local Mode**: Spawns process locally (no TCP)
- **Remote Mode**: Connects via TCP

This fixes the issue where local mode was still trying to establish TCP connections.

## Quick Migration

### If You Were Using Spawn Command:

**Before:**
```json
{
  "acp": {
    "spawn_command": "C:\\path\\to\\chat_cli.exe acp",
    "host": "127.0.0.1",
    "port": 8765
  }
}
```

**After (Local Mode):**
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

### If You Were Using External Process:

**Before:**
```json
{
  "acp": {
    "spawn_command": null,
    "host": "127.0.0.1",
    "port": 8765,
    "timeout_ms": 30000
  }
}
```

**After (Remote Mode):**
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

## Using Settings UI

The easiest way to migrate is through the Settings UI:

1. Open Kiro Assistant
2. Click system tray → Settings
3. Go to "Connection" section
4. Select your mode:
   - **Local (Spawn Process)** - If you want to spawn locally
   - **Remote (TCP Connection)** - If you want to connect to external server
5. Fill in the appropriate fields
6. Save and restart

## Key Differences

### Local Mode:
- ✅ No TCP connection attempted
- ✅ Process spawned and managed automatically
- ✅ No "Connection refused" errors
- ✅ No port configuration needed

### Remote Mode:
- ✅ Connects to external server
- ✅ Requires server to be running
- ✅ Supports remote hosts
- ✅ Connection retry logic

## Troubleshooting

### "Connection refused" Error in Local Mode

This error should no longer occur in local mode! If you see it:
1. Verify you're using Local mode in settings
2. Check config file has `"type": "local"`
3. Restart the application

### Migration Not Working

If settings don't load correctly:
1. Delete old config file
2. Restart application (creates new config)
3. Configure through Settings UI
4. Save and restart

Config file location:
- Windows: `%APPDATA%\kiro-assistant\config.json`
- Linux: `~/.config/kiro-assistant/config.json`
- macOS: `~/Library/Application Support/kiro-assistant/config.json`

## Benefits of New System

1. **Clear Separation**: Local and remote modes are distinct
2. **No Confusion**: Can't accidentally mix local spawn with TCP connection
3. **Better Errors**: Mode-specific error messages
4. **Simpler Config**: Only configure what you need
5. **Type Safety**: Rust enum ensures valid configurations

## Default Configuration

New installations default to Remote mode:
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

This maintains backward compatibility for users expecting to run external servers.

## Recommended Setup

For most users, Local mode is recommended:

1. Build your ACP server:
   ```bash
   cd kiro-cli
   cargo build --release
   ```

2. Configure Local mode in Settings:
   - Mode: Local (Spawn Process)
   - Spawn Command: `C:\path\to\chat_cli.exe acp`

3. Save and restart

4. Done! No TCP configuration needed.

# Quick Start: Using Spawn Command

## Setup

1. **Build your ACP server** (if using development build):
   ```bash
   cd kiro-cli
   cargo build --release
   ```

2. **Launch Kiro Assistant**

3. **Open Settings**:
   - Click the system tray icon
   - Select "Settings"

4. **Configure Spawn Command**:
   In the "Spawn Command" field, enter the full command to start your ACP server.

   **Windows Example:**
   ```
   C:\workplace\kiro-cli\target\release\chat_cli.exe acp
   ```

   **Linux/macOS Example:**
   ```
   /home/user/kiro-cli/target/release/chat_cli acp
   ```

5. **Save and Restart**:
   - Click "Save Settings"
   - Restart the assistant

## How to Find Your Binary Path

### Windows:
```powershell
# In your kiro-cli directory
cd target\release
pwd
# Copy the path and add \chat_cli.exe acp
```

### Linux/macOS:
```bash
# In your kiro-cli directory
cd target/release
pwd
# Copy the path and add /chat_cli acp
```

## Verification

After restart, check the logs to verify the process spawned:
- **Windows**: `%APPDATA%\kiro-assistant\logs\`
- **Linux**: `~/.config/kiro-assistant/logs/`
- **macOS**: `~/Library/Application Support/kiro-assistant/logs/`

Look for:
```
Spawn command configured: Some("C:\\path\\to\\chat_cli.exe acp")
Spawning Kiro process with command: C:\path\to\chat_cli.exe acp
Program: C:\path\to\chat_cli.exe, Args: ["acp"]
Kiro process spawned successfully
```

## Alternative: External Process

If you prefer to manage the ACP server manually:

1. Leave "Spawn Command" empty in settings
2. Start the ACP server manually:
   ```bash
   chat_cli acp --host 127.0.0.1 --port 8765
   ```
3. Launch Kiro Assistant (it will connect to the running server)

## Troubleshooting

### "Failed to spawn Kiro process"
- Verify the path is correct
- Check the binary exists at that location
- Ensure you included the full command (binary + arguments)

### "Failed to connect"
- Check the logs for spawn errors
- Verify the ACP server is actually starting
- Try running the command manually in a terminal first

### Process doesn't terminate
- Check logs for errors
- Manually kill: `taskkill /F /IM chat_cli.exe` (Windows) or `pkill chat_cli` (Linux/macOS)

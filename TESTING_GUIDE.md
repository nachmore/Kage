# Testing Guide for ACP Integration

## Prerequisites

1. Build the Kiro Assistant:
   ```bash
   cargo build --release
   ```

2. Build your ACP server (chat_cli):
   ```bash
   cd kiro-cli
   cargo build --release
   ```

## Test 1: Process Spawning (Hidden Window)

### Setup:
1. Open Kiro Assistant
2. Click system tray → Settings
3. In "Spawn Command" field, enter:
   ```
   C:\workplace\kiro-cli\target\release\chat_cli.exe acp
   ```
   (Adjust path to your actual build location)
4. Save settings
5. Restart Kiro Assistant

### Verify:
1. **No console window should appear** ✓
2. Open Task Manager (Windows) or Activity Monitor (Mac/Linux)
3. Look for `chat_cli.exe` or `chat_cli` process
4. Process should be running ✓
5. Check logs at `%APPDATA%\kiro-assistant\logs\` (Windows)
6. Should see:
   ```
   Spawn command configured: Some("C:\\workplace\\kiro-cli\\target\\release\\chat_cli.exe acp")
   Spawning Kiro process with command: ...
   Program: ..., Args: ["acp"]
   Kiro process spawned successfully
   ```

### Expected Result:
- ✅ No visible console window
- ✅ Process running in background
- ✅ Logs show successful spawn

## Test 2: Floating Window Chat

### Setup:
1. Ensure ACP server is running (from Test 1)
2. Press your hotkey (default: Alt+Space)
3. Floating window should appear

### Test Chat Message:
1. Type: "Hello, how are you?"
2. Press Enter
3. Observe:
   - Ghost should pulse (thinking animation) ✓
   - "Thinking..." text should appear ✓
   - Response should stream in character by character ✓
   - Ghost should stop pulsing when complete ✓
   - Expand button should appear ✓

### Test Multiple Messages:
1. Type another message
2. Press Enter while previous response is still streaming
3. Should be ignored (no double submission) ✓
4. Wait for first response to complete
5. Try second message again
6. Should work ✓

### Expected Result:
- ✅ Real-time streaming response
- ✅ Smooth animations
- ✅ No duplicate submissions
- ✅ Proper completion handling

## Test 3: Main Chat Window

### Setup:
1. Open floating window
2. Type a message and get a response
3. Click the expand button (↗)

### Verify:
1. Main chat window should open ✓
2. Floating window should hide ✓
3. Can send messages in main window ✓
4. Streaming works in main window ✓

### Expected Result:
- ✅ Seamless transition between windows
- ✅ Both windows use same ACP connection

## Test 4: Error Handling

### Test Connection Error:
1. Stop the ACP server manually:
   - Task Manager → End chat_cli.exe process
2. Open floating window
3. Type a message and press Enter
4. Should see error message ✓
5. Error should be clear and helpful ✓

### Test Reconnection:
1. Restart Kiro Assistant (will respawn ACP server)
2. Try sending a message again
3. Should work ✓

### Expected Result:
- ✅ Clear error messages
- ✅ Graceful failure handling
- ✅ Can recover after restart

## Test 5: App Launching (Existing Feature)

### Verify Still Works:
1. Open floating window
2. Type: "chrome" (or another installed app)
3. Should show app suggestions ✓
4. Press Enter to launch ✓
5. Window should hide ✓

### Expected Result:
- ✅ App launching still works
- ✅ Chat mode activates for non-app queries

## Test 6: Cross-Platform (If Available)

### Linux/macOS:
1. Build on Linux/macOS
2. Configure spawn command:
   ```
   /home/user/kiro-cli/target/release/chat_cli acp
   ```
3. Verify no terminal window appears ✓
4. Check process is detached from parent ✓
5. Test chat functionality ✓

### Expected Result:
- ✅ Works on all platforms
- ✅ No visible terminal/console

## Test 7: Process Cleanup

### Verify Proper Termination:
1. Open Task Manager/Activity Monitor
2. Note the chat_cli process ID
3. Close Kiro Assistant completely
4. Check Task Manager/Activity Monitor
5. chat_cli process should be gone ✓

### Expected Result:
- ✅ Process terminates with assistant
- ✅ No orphaned processes

## Test 8: Configuration Persistence

### Test Settings Saved:
1. Configure spawn command
2. Save settings
3. Close assistant
4. Reopen assistant
5. Open settings
6. Spawn command should still be there ✓

### Expected Result:
- ✅ Configuration persists across restarts

## Common Issues and Solutions

### Issue: "Failed to spawn Kiro process"
**Solution:**
- Verify path is correct
- Check binary exists at that location
- Ensure binary has execute permissions (Unix)
- Check logs for detailed error

### Issue: "Failed to connect to kiro-cli"
**Solution:**
- Verify process is actually running (Task Manager)
- Check if port 8765 is already in use
- Increase timeout in settings
- Check ACP server logs

### Issue: No response in floating window
**Solution:**
- Check browser console (F12 in dev mode)
- Verify event listeners are registered
- Check backend logs for errors
- Ensure ACP server is responding

### Issue: Console window still appears (Windows)
**Solution:**
- Verify you're using the release build
- Check CREATE_NO_WINDOW flag is being applied
- Try rebuilding with `cargo build --release`

## Logs Location

### Windows:
```
%APPDATA%\kiro-assistant\logs\
```

### Linux:
```
~/.config/kiro-assistant/logs/
```

### macOS:
```
~/Library/Application Support/kiro-assistant/logs/
```

## Success Criteria

All tests should pass:
- ✅ Process spawns without visible window
- ✅ ACP streaming works in floating window
- ✅ ACP streaming works in main window
- ✅ Error handling is graceful
- ✅ Process terminates cleanly
- ✅ Configuration persists
- ✅ Works on all platforms

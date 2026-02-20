# Hotkey Debugging Guide

## Problem
The global hotkey (Alt+Space) is not triggering the floating window.

## Debugging Steps

### Step 1: Check if the app is running
1. Close any existing instances of Kiro Assistant
2. Run the test script: `.\test_hotkey.ps1`
3. Look for these messages in the console:

```
=== KIRO ASSISTANT SETUP ===
Attempting to register global hotkey: Alt+Space
✅ Successfully registered global hotkey: Alt+Space
   Press Alt+Space to toggle the floating window
=== SETUP COMPLETE ===
```

### Step 2: Test the hotkey
1. With the app running, press `Alt+Space`
2. You should see this message in the console:

```
🔥 HOTKEY TRIGGERED: 14:23:45.123
  → Showing floating window
```

3. Press `Alt+Space` again:

```
🔥 HOTKEY TRIGGERED: 14:23:47.456
  → Hiding floating window
```

### Step 3: If hotkey doesn't trigger
If you don't see the "HOTKEY TRIGGERED" messages, it means:
- The hotkey is registered but Windows is not sending the event
- Another application is capturing Alt+Space first
- The hotkey registration failed silently

**Try these:**
1. Check if another app is using Alt+Space (common culprits: Spotlight alternatives, launcher apps)
2. Try the fallback hotkey `Alt+K` instead
3. Use the system tray icon to show the window manually

### Step 4: Test window visibility manually
1. Open the main chat window (from system tray)
2. Click the "🧪 Test Hotkey" button in the header
3. This will toggle the floating window without using the hotkey
4. If this works, the issue is with hotkey registration, not the window itself

### Step 5: Check the logs
1. Logs are written to: `%APPDATA%\kiro-assistant\logs\kiro.log`
2. Look for these entries:
   - `Attempting to register global hotkey`
   - `Successfully registered global hotkey` or `Failed to register`
   - `Hotkey triggered` (when you press the hotkey)

## Common Issues

### Issue 1: Hotkey registration fails
**Symptoms:** You see "❌ Failed to register Alt+Space" in the console

**Solutions:**
- Another app is using Alt+Space (close other launcher apps)
- Try Alt+K instead
- Change the hotkey in Settings

### Issue 2: Hotkey registers but doesn't trigger
**Symptoms:** You see "✅ Successfully registered" but no "HOTKEY TRIGGERED" messages

**Solutions:**
- Windows may be blocking the hotkey
- Try running as administrator
- Check Windows keyboard settings
- Restart the app

### Issue 3: Window doesn't show when hotkey triggers
**Symptoms:** You see "HOTKEY TRIGGERED" but window doesn't appear

**Solutions:**
- Window might be off-screen (check display settings)
- Window might be behind other windows
- Try the "Test Hotkey" button to verify window works

## Quick Test Commands

```powershell
# Kill existing instance
Stop-Process -Name "kiro-assistant" -Force

# Run with console output
.\target\debug\kiro-assistant.exe

# Check logs
Get-Content "$env:APPDATA\kiro-assistant\logs\kiro.log" -Tail 50
```

## Expected Console Output

When everything works correctly:
```
=== Kiro Assistant Starting ===
Configuration loaded: ACP host=localhost:8765
App launcher initialized
=== KIRO ASSISTANT SETUP ===
Attempting to register global hotkey: Alt+Space
✅ Successfully registered global hotkey: Alt+Space
   Press Alt+Space to toggle the floating window
=== SETUP COMPLETE ===
Active hotkey: Alt+Space
Floating window initial state: hidden

[Press Alt+Space]
🔥 HOTKEY TRIGGERED: 14:23:45.123
  → Showing floating window

[Press Alt+Space again]
🔥 HOTKEY TRIGGERED: 14:23:47.456
  → Hiding floating window
```

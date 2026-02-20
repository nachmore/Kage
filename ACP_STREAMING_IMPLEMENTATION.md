# ACP Streaming Implementation

## Changes Made

### 1. Hidden Process Spawning (Cross-Platform)

Updated `src/acp_client.rs` to spawn the ACP server process in the background without showing a console window.

#### Windows Implementation:
- Uses `CREATE_NO_WINDOW` flag to prevent console window from appearing
- Redirects stdin, stdout, stderr to null

#### Unix/Linux/macOS Implementation:
- Uses `setsid()` to create a new process group (detaches from parent)
- Redirects stdin, stdout, stderr to null
- Added `libc` dependency for Unix systems

#### Code Changes:
```rust
// Windows-specific: Hide the console window
#[cfg(target_os = "windows")]
{
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

// Unix-specific: Detach from parent process group
#[cfg(unix)]
{
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}
```

### 2. Floating Window ACP Integration

Updated `ui/floating.html` to properly handle ACP streaming instead of showing simulated responses.

#### Changes:
1. Added `listen` import from Tauri event API
2. Added event listeners for:
   - `message_chunk` - Receives streaming response chunks
   - `message_complete` - Notified when response is complete
   - `message_error` - Handles errors
3. Replaced simulated response with actual `send_message_streaming` call
4. Added `isWaitingForResponse` flag to prevent multiple submissions
5. Proper error handling and UI updates

#### Event Flow:
```
User types message → Enter key
  ↓
invoke('send_message_streaming', { message })
  ↓
Backend streams response chunks
  ↓
'message_chunk' events update UI in real-time
  ↓
'message_complete' event signals completion
  ↓
UI stops thinking animation, enables input
```

### 3. Dependencies

Added `libc` dependency for Unix systems in `Cargo.toml`:
```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

## How It Works

### Process Spawning:
1. When spawn command is configured, the assistant spawns the process
2. On Windows: Process runs without console window (invisible to user)
3. On Unix: Process detaches from parent and runs in background
4. All stdio is redirected to null (no output visible)
5. Process runs silently until assistant terminates it

### Message Flow:
1. User types in floating window and presses Enter
2. Floating window calls `send_message_streaming` Tauri command
3. Backend (Rust) sends message to ACP server via TCP
4. ACP server streams response back
5. Backend emits `message_chunk` events for each chunk
6. Floating window updates UI with each chunk
7. Backend emits `message_complete` when done
8. Floating window stops thinking animation

## Testing

### Test Process Spawning:
1. Configure spawn command in settings
2. Restart assistant
3. Verify no console window appears
4. Check logs to confirm process spawned successfully
5. Check Task Manager/Activity Monitor - process should be running

### Test ACP Streaming:
1. Open floating window (Alt+Space or configured hotkey)
2. Type a message that doesn't match any app
3. Press Enter
4. Should see "Thinking..." with pulsing ghost
5. Response should stream in character by character
6. Ghost should stop pulsing when complete
7. Can expand to full chat window

### Test Error Handling:
1. Stop ACP server manually
2. Try sending a message
3. Should see error message in floating window
4. Should be able to try again

## Platform Support

### Windows ✅
- Hidden console window using `CREATE_NO_WINDOW`
- Tested on Windows 10/11

### Linux ✅
- Process detachment using `setsid()`
- Should work on all Linux distributions

### macOS ✅
- Same Unix implementation as Linux
- Should work on macOS 10.15+

## Future Improvements

1. Add process health monitoring
2. Auto-restart if process crashes
3. Better error messages for spawn failures
4. Progress indicator for long responses
5. Cancel button for in-progress requests

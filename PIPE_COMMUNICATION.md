# Pipe Communication Implementation

## What Changed

Refactored ACP client to use **stdin/stdout pipes** for local mode instead of TCP connections.

## The Problem

Previously, even in local mode:
1. Process was spawned with stdin/stdout/stderr set to `null`
2. Client still tried to connect via TCP
3. Result: "Connection refused" errors and "Not connected" errors

## The Solution

### Local Mode Now Uses Pipes:
1. Spawn process with `stdin(Stdio::piped())` and `stdout(Stdio::piped())`
2. Communicate directly via pipes (no TCP)
3. JSON-RPC messages sent to stdin, responses read from stdout
4. stderr kept as `inherit()` for debugging

### Remote Mode Uses TCP:
1. Connects to external server via TCP
2. Standard TCP socket communication
3. Retry logic with exponential backoff

## Implementation Details

### Connection Enum:
```rust
enum Connection {
    Tcp(TcpStream),
    Pipe {
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    },
}
```

### Process Spawning (Local Mode):
```rust
let mut cmd = Command::new(program);
cmd.args(args)
    .stdin(Stdio::piped())   // ← For sending JSON
    .stdout(Stdio::piped())  // ← For receiving JSON
    .stderr(Stdio::inherit()); // ← For debugging

// Windows: Hide console window
#[cfg(target_os = "windows")]
{
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

let mut child = cmd.spawn()?;
let stdin = child.stdin.take()?;
let stdout = child.stdout.take()?;

// Store as pipe connection
Connection::Pipe {
    stdin,
    stdout: BufReader::new(stdout),
}
```

### Communication Flow:

**Sending:**
```rust
match connection {
    Connection::Tcp(stream) => {
        writeln!(stream, "{}", json)?;
        stream.flush()?;
    }
    Connection::Pipe { stdin, .. } => {
        writeln!(stdin, "{}", json)?;
        stdin.flush()?;
    }
}
```

**Receiving:**
```rust
match connection {
    Connection::Tcp(stream) => {
        let mut reader = BufReader::new(stream.try_clone()?);
        reader.read_line(&mut line)?;
    }
    Connection::Pipe { stdout, .. } => {
        stdout.read_line(&mut line)?;
    }
}
```

## Enhanced Logging

Added emoji-based logging for easy debugging:

- 🚀 Process spawning
- 📦 Program and args
- 🪟 Windows-specific flags
- 🐧 Unix-specific setup
- ✅ Success indicators
- ❌ Error indicators
- 📡 Pipe communication
- 🌐 TCP communication
- 📤 Sending messages
- 📥 Receiving responses
- 📝 JSON content
- 📨 Bytes read
- 📄 Response lines
- 📭 Stream end
- ⚠️  Warnings
- 💥 Fatal errors
- 🎉 Completion

## Benefits

### Local Mode:
- ✅ No TCP overhead
- ✅ No port conflicts
- ✅ No firewall issues
- ✅ Direct process communication
- ✅ Simpler architecture
- ✅ Better error messages

### Remote Mode:
- ✅ Still works as before
- ✅ TCP for network communication
- ✅ Retry logic intact

## Testing

### Check Logs:
Look for these indicators in logs:

**Local Mode Success:**
```
🚀 Spawning Kiro process with command: ...
📦 Program: ..., Args: [...]
✅ Process spawned successfully (PID: ...)
📡 Pipe handles acquired
🎉 Kiro process ready for communication
📡 Sending via pipe
✅ Pipe write successful
✅ Pipe flush successful
📥 Reading pipe response
📨 Read X bytes from pipe
📄 Response line: {...}
✅ Chat streaming completed
```

**Remote Mode Success:**
```
🌐 Remote mode: Establishing TCP connection
🔌 Attempting TCP connection to 127.0.0.1:8765
✅ Successfully connected to kiro-cli
🌐 Sending via TCP
✅ TCP write successful
📥 Reading TCP response
✅ Chat streaming completed
```

### Common Issues:

**"Not connected to kiro-cli"**
- Check mode is set to "local" in config
- Verify spawn command is correct
- Check logs for spawn errors

**"Pipe broken"**
- Process crashed or exited
- Check stderr output
- Verify binary is correct version

**"Failed to spawn"**
- Binary path incorrect
- Binary doesn't exist
- No execute permissions (Unix)

## Configuration

### Local Mode (Pipes):
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

### Remote Mode (TCP):
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

## Architecture

```
┌─────────────────────┐
│  Kiro Assistant     │
│  (Rust/Tauri)       │
└──────────┬──────────┘
           │
    ┌──────┴──────┐
    │             │
    ▼             ▼
┌────────┐   ┌────────┐
│ Local  │   │ Remote │
│  Mode  │   │  Mode  │
└───┬────┘   └───┬────┘
    │            │
    │ Pipes      │ TCP
    │            │
    ▼            ▼
┌────────┐   ┌────────┐
│ Child  │   │External│
│Process │   │ Server │
└────────┘   └────────┘
```

## Summary

Local mode now properly uses stdin/stdout pipes for communication, eliminating the need for TCP connections and resolving all "Connection refused" and "Not connected" errors. The implementation is clean, well-logged, and easy to debug.

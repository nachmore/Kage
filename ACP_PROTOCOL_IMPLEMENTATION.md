# ACP Protocol Implementation

## Overview

Implemented the proper Agent Client Protocol (ACP) flow based on the official specification at [agentclientprotocol.com](https://agentclientprotocol.com).

## Protocol Flow

### 1. Initialize Connection
First, we must initialize the connection with the agent:

```json
{
  "jsonrpc": "2.0",
  "id": 0,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientCapabilities": {
      "fs": {
        "readTextFile": true,
        "writeTextFile": true
      },
      "terminal": true
    },
    "clientInfo": {
      "name": "kiro-assistant",
      "title": "Kiro Assistant",
      "version": "0.1.0"
    }
  }
}
```

### 2. Create Session
After initialization, create a session:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "session/new",
  "params": {
    "cwd": "/absolute/path/to/working/directory",
    "mcpServers": []
  }
}
```

Response includes a `sessionId` that we use for all subsequent requests.

### 3. Send Prompts
Use the session ID to send prompts:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "session/prompt",
  "params": {
    "sessionId": "sess_abc123",
    "prompt": [
      {
        "type": "text",
        "text": "Your message here"
      }
    ]
  }
}
```

### 4. Receive Streaming Updates
The agent sends `session/update` notifications:

```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_abc123",
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": {
        "type": "text",
        "text": "Response text..."
      }
    }
  }
}
```

### 5. Completion
Finally, the agent responds to the original `session/prompt` request:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "stopReason": "end_turn"
  }
}
```

## Implementation Details

### New Types

**AcpNotification**: For handling `session/update` notifications
```rust
pub struct AcpNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
}
```

### New Methods

**initialize()**: Initializes the ACP connection
- Sends `initialize` request
- Negotiates protocol version and capabilities
- Marks connection as initialized

**create_session()**: Creates a new session
- Automatically initializes if not already done
- Sends `session/new` request
- Stores session ID for future use
- Uses current working directory by default

**send_chat_streaming()**: Sends prompts and streams responses
- Automatically creates session if needed
- Sends `session/prompt` request
- Listens for `session/update` notifications
- Accumulates `agent_message_chunk` content
- Calls callback with accumulated response
- Waits for final response with `stopReason`

### State Management

- `initialized`: Tracks if connection is initialized
- `session_id`: Stores current session ID
- Automatic initialization and session creation as needed

## Message Flow

```
Client                          Agent
  |                               |
  |--- initialize --------------->|
  |<-- initialize response -------|
  |                               |
  |--- session/new -------------->|
  |<-- session/new response ------|
  |    (with sessionId)           |
  |                               |
  |--- session/prompt ----------->|
  |<-- session/update ------------|  (notification)
  |<-- session/update ------------|  (notification)
  |<-- session/update ------------|  (notification)
  |<-- session/prompt response ---|  (final)
  |    (with stopReason)           |
```

## Key Changes from Old Implementation

### Before:
- Used custom `chat` method
- No initialization
- No session management
- Expected `content` and `done` fields in response

### After:
- Uses standard ACP protocol
- Proper initialization with `initialize`
- Session management with `session/new`
- Uses `session/prompt` for messages
- Handles `session/update` notifications
- Looks for `agent_message_chunk` updates
- Waits for `stopReason` in final response

## Enhanced Logging

All ACP operations are logged with emojis for easy debugging:

- 🔧 Initialization
- 🆕 Session creation
- 💬 Chat messages
- 📤 Sending requests
- 📥 Receiving responses
- 🔔 Notifications
- 📝 Content accumulation
- ✅ Success
- ❌ Errors

## Testing

The logs will show the complete ACP flow:

```
🔧 Initializing ACP connection
📤 Sending request: method=initialize, id=0
✅ ACP initialized successfully
🆕 Creating new ACP session
📤 Sending request: method=session/new, id=1
✅ Session created: sess_abc123
💬 Sending chat message (length: 25)
📤 Sending session/prompt
🔔 Notification: method=session/update
📝 Accumulated: 15 chars
🔔 Notification: method=session/update
📝 Accumulated: 42 chars
📬 Response: id=2
✅ Prompt completed
```

## Compatibility

This implementation follows the ACP specification version 1 and should work with any compliant ACP agent, including:
- GitHub Copilot CLI
- Zed Industries agents
- Custom ACP implementations

## References

- [ACP Protocol Overview](https://agentclientprotocol.com/protocol/overview)
- [Initialization](https://agentclientprotocol.com/protocol/initialization)
- [Session Setup](https://agentclientprotocol.com/protocol/session-setup)
- [Prompt Turn](https://agentclientprotocol.com/protocol/prompt-turn)

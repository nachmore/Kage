# Tool Permissions

## Overview

The Tool Permissions system provides user control over which tools the AI assistant can use. When the ACP (Agent Communication Protocol) requests permission to use a tool, the user is presented with options to approve or deny the request.

## User Experience

### Permission Modal

When the AI wants to use a tool (like web search, file operations, etc.), a modal appears with three options:

1. **Deny** - Reject this specific request
2. **Trust Once** - Allow this one time only
3. **Trust Always** - Remember this choice and auto-approve future requests for this tool

### Settings Panel

Users can manage tool permissions in Settings > Tool Permissions:

- **Trust All Tools** - Toggle to automatically approve all tool requests without prompting
  - Shows a warning about security implications
  - Useful for power users who trust the AI completely

- **Allowed Tools List** - Shows all tools granted "Trust Always" permission
  - Displays tool name and when permission was granted
  - Allows revoking permission for any tool

## Technical Implementation

### Backend (Rust)

**Config Structure** (`src/config.rs`):
```rust
pub struct ToolPermissionsConfig {
    pub trust_all: bool,
    pub allowed_tools: Vec<AllowedTool>,
}

pub struct AllowedTool {
    pub tool_call_id: String,
    pub title: String,
    pub allowed_at: String, // ISO 8601 timestamp
}
```

**ACP Client** (`src/acp_client.rs`):
- `send_chat_streaming()` now accepts an optional `permission_callback`
- Detects `session/request_permission` notifications from ACP
- Emits permission requests to the frontend via Tauri events

**Tauri Commands** (`src/main.rs`):
- `send_permission_response()` - Sends user's decision back to ACP
- `remove_tool_permission()` - Removes a tool from the allowed list

### Frontend (JavaScript)

**Permission Modal** (`ui/js/floating-permissions.js`):
- Listens for `permission_request` events from backend
- Shows modal with tool information
- Handles auto-approval for trusted tools
- Sends user's choice back to backend

**Settings Module** (`ui/js/settings/tool-permissions.js`):
- Manages trust_all toggle
- Displays and manages allowed tools list
- Allows revoking permissions

## Permission Flow

1. AI requests to use a tool
2. ACP sends `session/request_permission` notification
3. Backend checks if `trust_all` is enabled or tool is in allowed list
4. If auto-approved, sends `allow_once` response immediately
5. Otherwise, emits `permission_request` event to frontend
6. Frontend shows modal to user
7. User makes choice (deny/once/always)
8. Frontend calls `send_permission_response` command
9. Backend sends response to ACP
10. If "always", tool is added to config and saved

## Security Considerations

- **Trust All** is disabled by default and shows a warning when enabled
- Permissions are stored per-tool, not per-session
- Users can revoke permissions at any time
- Permission history includes timestamps for audit purposes

## Example ACP Permission Request

```json
{
  "jsonrpc": "2.0",
  "method": "session/request_permission",
  "params": {
    "sessionId": "054b43cd-e53e-4ea9-be6f-7e3db5a3395b",
    "toolCall": {
      "toolCallId": "tooluse_aAEsCgdNNuc0gpp1PkIhjz",
      "title": "Searching the web"
    },
    "options": [
      {"optionId": "allow_once", "name": "Yes", "kind": "allow_once"},
      {"optionId": "allow_always", "name": "Always", "kind": "allow_always"},
      {"optionId": "reject_once", "name": "No", "kind": "reject_once"}
    ]
  },
  "id": "b8c39264-fe39-49a9-9c5e-dd9e4934f3df"
}
```

## Future Enhancements

- Per-tool permission levels (read-only vs full access)
- Temporary time-based permissions
- Permission groups (e.g., "All file operations")
- Export/import permission settings
- Audit log of all tool usage

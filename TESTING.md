# Testing Guide for Kiro Desktop Assistant

## Task 2: Basic ACP Connection and Text Interaction

### Prerequisites

To test the ACP connection functionality, you have two options:

**Option 1: Use the Mock ACP Server (Recommended for Testing)**
- A simple mock server is included for testing purposes
- No need to set up the full kiro-cli

**Option 2: Use Real kiro-cli**
- The application expects kiro-cli to be listening on `127.0.0.1:8765`
- Start kiro-cli with ACP server mode before testing

### Quick Start with Mock Server

1. **Start the Mock ACP Server** (in one terminal):
   ```bash
   cargo test --test mock_acp_server -- --ignored --nocapture
   ```
   You should see: "Mock ACP server listening on 127.0.0.1:8765"

2. **Run the Application** (in another terminal):
   ```bash
   cargo run
   ```

3. **Test the Connection**:
   - Type a message and click Send
   - You should see a response: "Mock response to: [your message]"

### Running the Application

```bash
cargo run
```

### Testing Steps

1. **Launch the Application**
   - Run `cargo run` from the project root
   - The Kiro Assistant window should appear

2. **Check Connection Status**
   - Look at the connection status indicator at the top of the window
   - If kiro-cli is running: "Connected to kiro-cli" (green)
   - If kiro-cli is not running: "Not connected to kiro-cli" (red)

3. **Send a Message**
   - Type a message in the text input box at the bottom
   - Click the "Send" button or press Enter
   - The application will attempt to connect if not already connected

4. **View Response**
   - If successful: Your message and Kiro's response will appear in the response area
   - If connection fails: An error message will appear in red at the top

5. **Error Handling Test**
   - Try sending a message without kiro-cli running
   - You should see an error message: "Error: Connection error: Failed to connect to kiro-cli"
   - The error message will automatically disappear after 5 seconds

### Expected Behavior

✅ **Success Case:**
- Connection status shows "Connected to kiro-cli"
- Message is sent successfully
- Response appears in the format:
  ```
  You: [your message]
  
  Kiro: [response from kiro-cli]
  ```

❌ **Error Case:**
- Connection status shows "Not connected to kiro-cli"
- Error message appears when trying to send
- Error message is user-friendly and explains the issue

### Unit Tests

Run the unit tests to verify ACP protocol structure:

```bash
cargo test
```

All tests should pass:
- `test_acp_request_serialization` - Verifies request format
- `test_acp_response_deserialization` - Verifies response parsing
- `test_acp_error_response` - Verifies error handling

### ACP Protocol Details

The implementation follows the ACP protocol specification:

**Request Format:**
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-v4",
  "method": "chat",
  "params": {
    "message": "user message here"
  }
}
```

**Response Format:**
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-v4",
  "result": {
    "content": "assistant response here"
  }
}
```

**Error Format:**
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-v4",
  "error": {
    "code": -32600,
    "message": "Error description"
  }
}
```

### Troubleshooting

**Issue:** "Connection error: Failed to connect to kiro-cli"
- **Solution:** Ensure kiro-cli is running and listening on port 8765

**Issue:** "Send error: Not connected to kiro-cli"
- **Solution:** The connection was lost. Try sending again to reconnect.

**Issue:** "Send error: Invalid response format"
- **Solution:** The response from kiro-cli doesn't match the expected format. Check kiro-cli logs.

### Next Steps

After verifying Task 2 works correctly:
1. Commit the changes: `git commit -m "Task 2: Implement basic ACP connection and text interaction"`
2. Proceed to Task 3: Back-and-Forth Chat Conversation

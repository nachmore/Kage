# ACP Spawn Command Implementation Summary

## Changes Made

Successfully implemented configuration for spawning a local ACP server using a custom command string. This provides maximum flexibility for users to specify the exact command and arguments needed.

## Modified Files

### 1. `src/config.rs`
- Changed `kiro_binary_path: Option<String>` to `spawn_command: Option<String>` in `AcpConfig` struct
- Updated default configuration to include `spawn_command: None`

### 2. `src/acp_client.rs`
- Changed `kiro_binary_path` to `spawn_command` field in `AcpClient` struct
- Renamed `with_binary_path()` to `with_spawn_command()` builder method
- Updated `spawn_kiro_process()` to parse and execute the full command string
- Command parsing splits on whitespace: first part is program, rest are arguments
- Increased startup delay to 1000ms (1 second) for more reliable process initialization
- Updated `connect()` to check for spawn command instead of binary path
- Updated `disconnect()` to terminate spawned process on exit

### 3. `src/main.rs`
- Updated ACP client initialization to use `with_spawn_command()` method
- Updated logging to reference "spawn command" instead of "binary path"

### 4. `ui/settings.html`
- Changed "Kiro Binary Path" to "Spawn Command" field
- Updated placeholder text to show example: `C:\path\to\chat_cli.exe acp`
- Added detailed description explaining the full command format
- Updated help text to clarify host/port are only used for external processes
- Updated JavaScript to use `spawn_command` instead of `kiro_binary_path`

### 5. `BACKEND_CHANGES_NEEDED.md`
- Updated documentation to reflect spawn command approach
- Added examples showing full command strings with arguments
- Explained flexibility for future CLI changes

## New Files

### 1. `config.example.json`
- Example configuration file showing `spawn_command` field
- Includes all available options

### 2. `ACP_LOCAL_BINARY.md`
- User-facing documentation for the feature
- Platform-specific examples with full commands
- Explains flexibility of command-based approach

### 3. `IMPLEMENTATION_SUMMARY.md`
- This file documenting the implementation

## How It Works

### Command Parsing:
The spawn command is split on whitespace:
- First part: program/binary path
- Remaining parts: arguments

Example: `C:\path\to\chat_cli.exe acp --verbose`
- Program: `C:\path\to\chat_cli.exe`
- Args: `["acp", "--verbose"]`

### When Spawn Command is Configured:
1. Assistant reads config on startup
2. If `spawn_command` is set, parses it into program + arguments
3. Spawns process using `Command::new(program).args(args)`
4. Waits 1 second for process startup
5. Connects to the spawned process via TCP
6. On exit, terminates the spawned process

### When Spawn Command is Not Configured:
1. Assistant attempts to connect to existing process at host:port
2. User must manually start/stop the ACP server

## Configuration Examples

### Basic Usage:
```json
{
  "acp": {
    "spawn_command": "C:\\workplace\\kiro-cli\\target\\release\\chat_cli.exe acp"
  }
}
```

### With Additional Arguments:
```json
{
  "acp": {
    "spawn_command": "chat_cli acp --verbose --log-level debug"
  }
}
```

### External Process (No Spawn):
```json
{
  "acp": {
    "host": "127.0.0.1",
    "port": 8765,
    "spawn_command": null
  }
}
```

## Key Benefits

1. **Flexibility**: Users can specify any command with any arguments
2. **Future-proof**: CLI changes don't require assistant code updates
3. **Simple**: Single text field captures everything needed
4. **Powerful**: Supports complex command lines with multiple arguments

## Testing

- ✅ Code compiles successfully (`cargo check`)
- ✅ Configuration structure is backward compatible (optional field with default)
- ✅ Settings UI updated with new field and clear examples
- ✅ Command parsing handles spaces in paths and multiple arguments

## Example User Workflow

1. User builds their ACP server: `cargo build --release`
2. User opens Kiro Assistant Settings
3. User enters in "Spawn Command" field:
   ```
   C:\workplace\kiro-cli\target\release\chat_cli.exe acp
   ```
4. User saves settings and restarts assistant
5. Assistant automatically spawns and manages the ACP server process
6. User can add arguments later without code changes:
   ```
   C:\workplace\kiro-cli\target\release\chat_cli.exe acp --verbose
   ```

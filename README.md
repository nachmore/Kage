# amzn-kiro-assistant

_TODO: Describe the package, its customers and owner. You should also include the recommended channels for support and a CTI for any bugs._

_TODO: If this is a library package, consider linking to the rustdoc build artifact for the Version Set your consumers will be consuming this package from. [Example link](https://code.amazon.com/packages/YourPackageName/releases/0.1/latest_artifact?version_set=YourPackageName/development&path=brazil-documentation/your_crate_name/index.html). [More information](https://docs.hub.amazon.dev/languages/rust/cargobrazil/#accessing-produced-documentation)_

## Useful links

- [Code Browser](https://code.amazon.com/packages/Kiro-Assistant/)

## Development Mode

To run the application in development mode with additional debugging features, use the `/dev` flag:

```bash
cargo run -- /dev
```

Or with the built executable:

```bash
./kiro-assistant /dev
```

When running in dev mode, the system tray menu will include two additional options:

- **Inspect**: Opens the developer tools/inspector window for debugging the UI
- **Reload UX**: Reloads all HTML content in the application windows without restarting the app

## Debug Mode

To enable detailed ACP (Agent Communication Protocol) logging to the console, use the `/debug` flag:

```bash
cargo run -- /debug
```

Or with the built executable:

```bash
./kiro-assistant /debug
```

Debug mode prints all ACP messages (requests, responses, and streaming updates) to the console with timestamps. This is useful for:
- Troubleshooting connection issues with kiro-cli
- Understanding the ACP protocol flow
- Debugging message format problems
- Development and testing

You can combine both flags:
```bash
./kiro-assistant /dev /debug
```

For more details, see [Debug Mode Guide](docs/DEBUG_MODE.md).
 

## Features

### Shortcuts

Kiro supports custom command shortcuts that allow you to execute commands directly from the floating window without sending them to the LLM. This is perfect for quick access to frequently used tools, scripts, or applications.

**Key Features:**
- Execute commands instantly by typing a trigger word
- Pass dynamic arguments using `{*}` or `{0}`, `{1}`, etc.
- Visual feedback in the suggestion dropdown
- Import/Export shortcuts as JSON
- Full management UI in settings

**Quick Example:**
```json
{
  "name": "Open VSCode",
  "shortcut": "code",
  "path": "C:\\Program Files\\Microsoft VS Code\\Code.exe",
  "arguments": "{*}"
}
```

Type `code myproject` in the floating window to instantly open VSCode with your project.

For detailed documentation, see [Shortcuts Guide](docs/SHORTCUTS_GUIDE.md).

## Documentation

- [Debug Mode Guide](docs/DEBUG_MODE.md) - Enable detailed ACP logging for troubleshooting
- [Shortcuts Guide](docs/SHORTCUTS_GUIDE.md) - Complete guide to using and configuring shortcuts
- [OS Abstraction Guide](docs/OS_ABSTRACTION_GUIDE.md) - Cross-platform OS abstraction layer
- [OS Architecture](docs/OS_ARCHITECTURE.md) - System architecture documentation

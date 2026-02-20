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
 

//! kage-core — shared foundation for the `kage` app and the
//! `kage-computer-control-mcp` sidecar.
//!
//! Everything here is Tauri-free by design: the sidecar links this crate
//! instead of the full app, keeping its dependency graph tiny (seconds to
//! build, megabytes on disk) and letting CI build the app graph once per
//! platform instead of twice. Don't add Tauri, webview, or app-state
//! dependencies to this crate — command wrappers belong in `kage`.

pub mod computer_control;
pub mod folder_tools;
pub mod lock_ext;
pub mod mcp_json_rpc;
pub mod os;

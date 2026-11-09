//! kage-core — shared foundation for the `kage` app and the
//! `kage-computer-control-mcp` sidecar.
//!
//! Everything here is Tauri-free by design: the sidecar links this crate
//! instead of the full app, keeping its dependency graph tiny (seconds to
//! build, megabytes on disk) and letting CI build the app graph once per
//! platform instead of twice. Don't add Tauri, webview, or app-state
//! dependencies to this crate — command wrappers belong in `kage`.
//!
//! The `os` feature (on by default) gates the OS layer and the modules
//! built on it. Without it only the plain data modules remain, which is
//! all the `kage-file-text` worker links.

pub mod calendar;
#[cfg(feature = "os")]
pub mod changelog_cache;
#[cfg(feature = "os")]
pub mod computer_control;
pub mod file_text_wire;
#[cfg(feature = "os")]
pub mod folder_tools;
pub mod host_bridge;
pub mod lock_ext;
pub mod mcp_json_rpc;
#[cfg(feature = "os")]
pub mod os;
#[cfg(feature = "os")]
pub mod paths;

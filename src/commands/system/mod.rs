//! System-level commands. Split from a single 2,800-line file into
//! topical submodules. See sub-mod docs for what lives where.
//!
//! All public items are re-exported here so existing call sites
//! (`crate::commands::system::X`) and the top-level
//! `crate::commands::X` re-export keep working unchanged.

pub mod agents;
pub mod config_io;
pub mod diagnostics;
pub mod hotkeys;
pub mod integrations;
pub mod lifecycle;
pub mod privacy_permissions;
pub mod steering;
pub mod updates;
/// Commands the user runs themselves from a sandbox notice. Not glob
/// re-exported: the registry names them by full path.
pub mod user_shell;

pub use agents::*;
pub use config_io::*;
pub use diagnostics::*;
pub use hotkeys::*;
pub use integrations::*;
pub use lifecycle::*;
pub use privacy_permissions::*;
pub use steering::*;
pub use updates::*;

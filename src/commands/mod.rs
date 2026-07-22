pub mod agent_sessions;
pub mod extensions;
pub mod folder_tools;
pub mod i18n;
pub mod input;
pub mod kiro_desktop;
pub mod messaging;
pub mod oauth;
pub mod pocket_tts;
pub mod registry;
pub mod sessions;
pub mod system;
pub mod window;

// Re-export all commands for convenient registration in main.rs
pub use agent_sessions::*;
pub use extensions::*;
pub use folder_tools::*;
pub use i18n::*;
pub use input::*;
pub use kiro_desktop::*;
pub use messaging::*;
pub use oauth::*;
pub use pocket_tts::*;
pub use sessions::*;
pub use system::*;
pub use window::*;

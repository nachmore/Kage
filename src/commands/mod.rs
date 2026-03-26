pub mod extensions;
pub mod folder_tools;
pub mod input;
pub mod kage_desktop;
pub mod messaging;
pub mod pocket_tts;
pub mod sessions;
pub mod system;
pub mod window;

// Re-export all commands for convenient registration in main.rs
pub use extensions::*;
pub use folder_tools::*;
pub use input::*;
pub use kage_desktop::*;
pub use messaging::*;
pub use pocket_tts::*;
pub use sessions::*;
pub use system::*;
pub use window::*;

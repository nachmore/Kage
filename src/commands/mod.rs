pub mod extensions;
pub mod input;
pub mod messaging;
pub mod sessions;
pub mod system;
pub mod window;

// Re-export all commands for convenient registration in main.rs
pub use extensions::*;
pub use input::*;
pub use messaging::*;
pub use sessions::*;
pub use system::*;
pub use window::*;

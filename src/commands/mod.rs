pub mod input;
pub mod messaging;
pub mod system;
pub mod window;

// Re-export all commands for convenient registration in main.rs
pub use input::*;
pub use messaging::*;
pub use system::*;
pub use window::*;

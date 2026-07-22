// Linux-specific implementations (kage-core subset).
//
// accessibility and input are stubs returning "unsupported" — Linux
// computer-control isn't implemented yet. launcher has a real
// .desktop-file scanner.

pub mod accessibility;
pub mod input;
pub mod launcher;
pub mod process;

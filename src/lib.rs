// Library exports for testing and external use

// Pure modules — always available, including in test mode
pub mod agent_presets;
pub mod config;
pub mod acp_client;
pub mod activity_tracker;
pub mod app_launcher;
pub mod app_log;
pub mod auto_steering;
pub mod computer_control;
pub mod error;
pub mod extensions;
pub mod logger;
pub mod mcp_registration;
pub mod os;
pub mod process_manager;

// Tauri-dependent modules — excluded from test compilation because
// Tauri's type system doesn't support --test mode.
#[cfg(not(test))]
pub mod automation;
#[cfg(not(test))]
pub mod commands;
#[cfg(not(test))]
pub mod single_instance;
#[cfg(not(test))]
pub mod state;
#[cfg(not(test))]
pub mod tray;
#[cfg(not(test))]
pub mod updater;

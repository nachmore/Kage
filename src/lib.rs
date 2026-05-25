// Library exports for testing and external use

// Pure modules — always available, including in test mode
pub mod acp_client;
pub mod activity_tracker;
pub mod agent_presets;
pub mod app_launcher;
pub mod app_log;
pub mod auto_steering;
pub mod chunk_batcher;
pub mod computer_control;
pub mod config;
pub mod config_export;
pub mod config_migrations;
pub mod error;
pub mod extensions;
pub mod link_metadata_cache;
pub mod lock_ext;
pub mod logger;
pub mod mcp_json_rpc;
pub mod mcp_registration;
pub mod ollama;
pub mod os;
pub mod panic_handler;
pub mod permission_audit;
pub mod process_manager;
pub mod startup;
pub mod steering_io;

// Tauri-dependent modules — excluded from test compilation because
// Tauri's type system doesn't support --test mode.
#[cfg(not(test))]
pub mod automation;
#[cfg(not(test))]
pub mod commands;
#[cfg(not(test))]
pub mod setup;
#[cfg(not(test))]
pub mod state;
#[cfg(not(test))]
pub mod telemetry;
#[cfg(not(test))]
pub mod tray;
#[cfg(not(test))]
pub mod updater;

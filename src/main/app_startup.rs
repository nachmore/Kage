use crate::{logger, panic_handler, startup, webview_recovery};
use log::info;
use std::time::Instant;

pub struct Context {
    pub args: Vec<String>,
    pub dev_mode: bool,
    pub debug_mode: bool,
    pub started_at: Instant,
}

/// Performs the process-wide startup work that must precede Tauri builder
/// construction, including restart cleanup and optional debug logging.
pub fn initialize() -> Context {
    panic_handler::install();
    if let Err(e) = logger::init_logger() {
        eprintln!("Failed to initialize logger: {}", e);
        eprintln!("Continuing without file logging...");
    }

    webview_recovery::init_at_startup();
    info!("=== Kage Starting ===");
    let started_at = Instant::now();

    let args: Vec<String> = std::env::args().collect();
    let flags = startup::CliFlags::parse(&args);
    startup::wait_for_previous_instance_if_restart(flags.is_restart);
    startup::ensure_webview_directory_writable();

    if flags.debug_mode {
        println!("🐛 DEBUG MODE ENABLED - Detailed ACP logs will be printed to console");
        info!("🐛 DEBUG MODE ENABLED via command line argument");
        logger::enable_console_logging();
    }
    if flags.dev_mode {
        info!(
            "⏱ Tauri builder starting at +{}ms",
            started_at.elapsed().as_millis()
        );
    }

    Context {
        args,
        dev_mode: flags.dev_mode,
        debug_mode: flags.debug_mode,
        started_at,
    }
}

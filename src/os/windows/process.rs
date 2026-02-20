// Windows process management

use anyhow::Result;
use log::info;
use std::process::Command;
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

pub fn kill_process_impl(pid: u32) -> bool {
    // Use taskkill on Windows
    match Command::new("taskkill")
        .args(&["/F", "/PID", &pid.to_string()])
        .output()
    {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

pub fn configure_spawn_impl(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
    info!("Windows: Setting CREATE_NO_WINDOW flag");
}

pub fn install_signal_handlers_impl<F>(cleanup_fn: F) -> Result<()>
where
    F: Fn() + Send + 'static,
{
    ctrlc::set_handler(move || {
        info!("Received Ctrl+C signal");
        cleanup_fn();
        std::process::exit(0);
    })?;
    
    info!("✅ Ctrl+C handler installed");
    Ok(())
}

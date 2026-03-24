// macOS process management

use anyhow::Result;
use log::info;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;
use std::process::Command;

pub fn get_process_name_impl(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

pub fn kill_process_impl(pid: u32) -> bool {
    // Try SIGTERM first
    if kill(Pid::from_raw(pid as i32), Signal::SIGTERM).is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        // Force kill if still alive
        if kill(Pid::from_raw(pid as i32), Signal::SIGKILL).is_ok() {
            return true;
        }
    }
    false
}

pub fn configure_spawn_impl(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            // Create new process group
            libc::setsid();
            Ok(())
        });
    }
    info!("macOS: Setting up process detachment");
}

pub fn install_signal_handlers_impl<F>(cleanup_fn: F) -> Result<()>
where
    F: Fn() + Send + 'static,
{
    std::thread::spawn(move || {
        let mut signals = Signals::new(&[SIGTERM, SIGINT, SIGQUIT])
            .expect("Failed to register signal handlers");
        
        for sig in signals.forever() {
            info!("Received signal: {:?}", sig);
            cleanup_fn();
            std::process::exit(0);
        }
    });
    
    info!("✅ Signal handlers installed (SIGTERM, SIGINT, SIGQUIT)");
    Ok(())
}

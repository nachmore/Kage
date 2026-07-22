// Linux process helpers (kage-core subset — see os/process.rs).

use std::process::Command;

pub fn get_process_name_impl(pid: u32) -> Option<String> {
    // Try /proc/{pid}/comm first (no subprocess needed)
    if let Ok(name) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
        let name = name.trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    // Fallback to ps
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Spawn a process detached from the parent. On Linux this is a plain
/// `Command::spawn` since there's no Windows-style Job Object to break
/// out of.
pub fn spawn_detached_impl(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    cmd.spawn()
}

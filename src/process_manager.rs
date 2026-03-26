use anyhow::{Context, Result};
use log::{info, warn};
use std::fs;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};

use crate::os;

/// Manages spawned CLI processes with cleanup on exit
pub struct ProcessManager {
    child: Arc<Mutex<Option<Child>>>,
    pid_file: PathBuf,
    pid: Option<u32>,
}

impl Default for ProcessManager {
    fn default() -> Self { Self::new() }
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new() -> Self {
        let pid_file = Self::get_pid_file_path();
        Self {
            child: Arc::new(Mutex::new(None)),
            pid_file,
            pid: None,
        }
    }

    /// Get the path to the PID file
    fn get_pid_file_path() -> PathBuf {
        let mut path = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."));
        path.push("kage");
        
        // Create directory if it doesn't exist
        if let Err(e) = fs::create_dir_all(&path) {
            warn!("Failed to create PID directory {:?}: {}", path, e);
        }
        
        path.push("spawned_cli.pid");
        path
    }

    /// Clean up any orphaned processes from previous runs
    pub fn cleanup_orphaned_processes() -> Result<()> {
        let pid_file = Self::get_pid_file_path();
        
        if !pid_file.exists() {
            info!("No PID file found, no orphaned processes to clean up");
            return Ok(());
        }

        match fs::read_to_string(&pid_file) {
            Ok(content) => {
                if let Ok(pid) = content.trim().parse::<u32>() {
                    info!("Found PID file with PID: {}", pid);
                    
                    // Verify the PID still belongs to a kage-related process
                    // to avoid killing a recycled PID that now belongs to something else
                    match os::process::get_process_name(pid) {
                        Some(name) => {
                            let name_lower = name.to_lowercase();
                            let is_ours = name_lower.contains("Kage")
                                || name_lower.contains("node")
                                || name_lower.contains("npx");
                            
                            if is_ours {
                                info!("PID {} is '{}' — killing orphaned process", pid, name);
                                if Self::kill_process(pid) {
                                    info!("✅ Cleaned up orphaned process (PID: {}, name: {})", pid, name);
                                } else {
                                    warn!("Failed to kill orphaned process (PID: {}, name: {})", pid, name);
                                }
                            } else {
                                info!("PID {} is '{}' — not a kage process, skipping kill (PID was recycled)", pid, name);
                            }
                        }
                        None => {
                            info!("PID {} is not running (already exited)", pid);
                        }
                    }
                }
                
                // Remove the PID file
                let _ = fs::remove_file(&pid_file);
                info!("PID file removed");
            }
            Err(e) => {
                warn!("Failed to read PID file: {}", e);
                let _ = fs::remove_file(&pid_file);
            }
        }

        Ok(())
    }

    /// Store a spawned child process
    pub fn store_process(&mut self, child: Child) -> Result<()> {
        let pid = child.id();
        info!("Storing process with PID: {}", pid);
        
        // Write PID to file
        fs::write(&self.pid_file, pid.to_string())
            .context("Failed to write PID file")?;
        
        self.pid = Some(pid);
        *self.child.lock().unwrap() = Some(child);
        
        info!("✅ Process registered for cleanup (PID: {})", pid);
        Ok(())
    }

    /// Kill a process by PID
    fn kill_process(pid: u32) -> bool {
        os::kill_process(pid)
    }

    /// Terminate the managed process
    pub fn terminate(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let pid = child.id();
            info!("Terminating spawned process (PID: {})", pid);
            
            // Try graceful shutdown first
            let _ = child.kill();
            
            // Wait for process to exit (with timeout)
            let wait_result = std::thread::spawn(move || {
                child.wait()
            }).join();
            
            if wait_result.is_ok() {
                info!("✅ Process terminated gracefully");
            } else {
                warn!("Process may not have terminated cleanly");
                
                // Force kill if still running
                if let Some(pid) = self.pid {
                    Self::kill_process(pid);
                }
            }
        }
        
        // Clean up PID file
        if self.pid_file.exists() {
            let _ = fs::remove_file(&self.pid_file);
            info!("✅ PID file removed");
        }
        
        self.pid = None;
    }

}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        info!("ProcessManager dropping, cleaning up...");
        self.terminate();
    }
}

/// Install signal handlers for graceful shutdown
pub fn install_signal_handlers(process_manager: Arc<Mutex<ProcessManager>>) {
    let cleanup = move || {
        if let Ok(mut pm) = process_manager.lock() {
            pm.terminate();
        }
    };
    
    if let Err(e) = os::process::install_signal_handlers(cleanup) {
        warn!("Failed to install signal handlers: {}", e);
    }
}

use anyhow::{Context, Result};
use log::{info, warn};
use std::fs;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};

/// Manages spawned CLI processes with cleanup on exit
pub struct ProcessManager {
    child: Arc<Mutex<Option<Child>>>,
    pid_file: PathBuf,
    pid: Option<u32>,
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
        path.push("kiro-assistant");
        
        // Create directory if it doesn't exist
        let _ = fs::create_dir_all(&path);
        
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
                    
                    // Try to kill the process
                    if Self::kill_process(pid) {
                        info!("✅ Cleaned up orphaned process (PID: {})", pid);
                    } else {
                        info!("Process {} not running (already cleaned up)", pid);
                    }
                }
                
                // Remove the PID file
                let _ = fs::remove_file(&pid_file);
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
    #[cfg(target_os = "windows")]
    fn kill_process(pid: u32) -> bool {
        use std::process::Command;
        
        // Use taskkill on Windows
        match Command::new("taskkill")
            .args(&["/F", "/PID", &pid.to_string()])
            .output()
        {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn kill_process(pid: u32) -> bool {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        
        // Try SIGTERM first
        if kill(Pid::from_raw(pid as i32), Signal::SIGTERM).is_ok() {
            std::thread::sleep(std::time::Duration::from_millis(500));
            
            // Check if process is still alive
            if kill(Pid::from_raw(pid as i32), Signal::SIGKILL).is_ok() {
                return true;
            }
        }
        
        false
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

    /// Check if the managed process is still running
    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            // Try to check if process is still alive
            match child.try_wait() {
                Ok(Some(_)) => false, // Process has exited
                Ok(None) => true,     // Process is still running
                Err(_) => false,      // Error checking, assume not running
            }
        } else {
            false
        }
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        info!("ProcessManager dropping, cleaning up...");
        self.terminate();
    }
}

/// Install signal handlers for graceful shutdown
#[cfg(not(target_os = "windows"))]
pub fn install_signal_handlers(process_manager: Arc<Mutex<ProcessManager>>) {
    use signal_hook::consts::signal::*;
    use signal_hook::iterator::Signals;
    
    std::thread::spawn(move || {
        let mut signals = Signals::new(&[SIGTERM, SIGINT, SIGQUIT])
            .expect("Failed to register signal handlers");
        
        for sig in signals.forever() {
            info!("Received signal: {:?}", sig);
            
            // Clean up process
            if let Ok(mut pm) = process_manager.lock() {
                pm.terminate();
            }
            
            // Exit the application
            std::process::exit(0);
        }
    });
    
    info!("✅ Signal handlers installed (SIGTERM, SIGINT, SIGQUIT)");
}

#[cfg(target_os = "windows")]
pub fn install_signal_handlers(process_manager: Arc<Mutex<ProcessManager>>) {
    use ctrlc;
    
    ctrlc::set_handler(move || {
        info!("Received Ctrl+C signal");
        
        // Clean up process
        if let Ok(mut pm) = process_manager.lock() {
            pm.terminate();
        }
        
        // Exit the application
        std::process::exit(0);
    }).expect("Failed to set Ctrl+C handler");
    
    info!("✅ Ctrl+C handler installed");
}

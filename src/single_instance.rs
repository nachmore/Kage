//! Single-instance enforcement using an OS-level file lock.
//!
//! Both debug and release builds use the same lock file in the shared
//! config directory (`kiro-assistant/`), so only one instance can run
//! regardless of build profile.

use anyhow::{Context, Result};
use log::info;
use std::fs::{self, File};
use std::path::PathBuf;

/// Holds the lock file handle. The OS lock is released when this is dropped
/// (or when the process exits/crashes).
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        info!("Releasing single-instance lock: {:?}", self.path);
    }
}

/// Try to acquire the single-instance lock.
/// Returns `Ok(InstanceLock)` if we're the only instance, or an error
/// describing that another instance is already running.
/// If `wait` is true, retries for up to 30 seconds (used during restart).
pub fn try_acquire(wait: bool) -> Result<InstanceLock> {
    let config_dir = dirs::config_dir()
        .context("Failed to get config directory")?
        .join("kiro-assistant");

    fs::create_dir_all(&config_dir)
        .context("Failed to create config directory")?;

    let lock_path = config_dir.join("instance.lock");

    let max_attempts = if wait { 60 } else { 1 }; // 60 x 500ms = 30s
    for attempt in 0..max_attempts {
        if attempt > 0 {
            if attempt == 1 {
                info!("Waiting for previous instance to exit...");
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        let file = File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open lock file: {:?}", lock_path))?;

        match try_lock_file(&file)? {
            true => {
                // Got the lock — write our PID
                use std::io::Write;
                let mut f = &file;
                let _ = f.write_all(format!("{}", std::process::id()).as_bytes());
                info!("Single-instance lock acquired: {:?}", lock_path);
                return Ok(InstanceLock { _file: file, path: lock_path });
            }
            false => {
                if attempt % 4 == 0 {
                    info!("Lock still held (attempt {}/{}), checking if stale...", attempt + 1, max_attempts);
                }
            }
        }

        // Lock is held — check if stale
        if is_lock_stale(&lock_path) {
            info!("Stale lock detected, overriding...");
            drop(file);
            let _ = fs::remove_file(&lock_path);
            let file = File::options()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&lock_path)
                .with_context(|| format!("Failed to recreate lock file: {:?}", lock_path))?;
            if try_lock_file(&file)? {
                use std::io::Write;
                let mut f = &file;
                let _ = f.write_all(format!("{}", std::process::id()).as_bytes());
                info!("Single-instance lock acquired (after stale override): {:?}", lock_path);
                return Ok(InstanceLock { _file: file, path: lock_path });
            }
        }
    }

    anyhow::bail!(
        "Another instance of Kiro Assistant is already running.\n\
         Lock file: {:?}",
        lock_path
    );
}

// --- Platform-specific locking ---

/// Check if the lock file contains a PID of a process that's no longer running.
fn is_lock_stale(lock_path: &std::path::Path) -> bool {
    let content = match fs::read_to_string(lock_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let pid: u32 = match content.trim().parse() {
        Ok(p) => p,
        Err(_) => return true, // Can't parse PID — treat as stale
    };
    // Don't consider our own PID as stale
    if pid == std::process::id() {
        return false;
    }
    !is_process_running(pid)
}

#[cfg(windows)]
fn is_process_running(pid: u32) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::Foundation::CloseHandle;
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn try_lock_file(file: &File) -> Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows::Win32::System::IO::OVERLAPPED;

    let handle = windows::Win32::Foundation::HANDLE(file.as_raw_handle());
    let mut overlapped = OVERLAPPED::default();

    let result = unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            Some(0),
            1,
            0,
            &mut overlapped,
        )
    };

    match result {
        Ok(()) => Ok(true),
        Err(e) => {
            // ERROR_LOCK_VIOLATION means another process holds the lock
            let code = e.code().0 as u32;
            if code == 0x80070021 {
                // HRESULT for ERROR_LOCK_VIOLATION
                Ok(false)
            } else {
                Err(e).context("LockFileEx failed")
            }
        }
    }
}

#[cfg(unix)]
fn try_lock_file(file: &File) -> Result<bool> {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

    if result == 0 {
        Ok(true)
    } else {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(err).context("flock failed")
        }
    }
}

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
pub fn try_acquire() -> Result<InstanceLock> {
    let config_dir = dirs::config_dir()
        .context("Failed to get config directory")?
        .join("kiro-assistant");

    fs::create_dir_all(&config_dir)
        .context("Failed to create config directory")?;

    let lock_path = config_dir.join("instance.lock");

    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("Failed to open lock file: {:?}", lock_path))?;

    if !try_lock_file(&file)? {
        anyhow::bail!(
            "Another instance of Kiro Assistant is already running.\n\
             Lock file: {:?}",
            lock_path
        );
    }

    // Write our PID into the lock file for diagnostics
    use std::io::Write;
    let mut f = &file;
    let _ = f.write_all(format!("{}", std::process::id()).as_bytes());

    info!("Single-instance lock acquired: {:?}", lock_path);

    Ok(InstanceLock {
        _file: file,
        path: lock_path,
    })
}

// --- Platform-specific locking ---

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

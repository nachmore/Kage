//! Windows thread sampling via Toolhelp + `GetThreadTimes`. Used by the
//! debug dump-thread-info command.

use crate::os::diagnostics::ThreadSample;
use log::error;

pub fn supports_thread_sampling_impl() -> bool {
    true
}

pub fn sample_threads_impl() -> Vec<ThreadSample> {
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::*;

    let pid = std::process::id();
    let mut threads = Vec::new();

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    let snapshot = match snapshot {
        Ok(h) => h,
        Err(e) => {
            error!("Failed to create thread snapshot: {}", e);
            return threads;
        }
    };

    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };

    unsafe {
        if Thread32First(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32OwnerProcessID == pid {
                    if let Ok(handle) = OpenThread(
                        THREAD_QUERY_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION,
                        false,
                        entry.th32ThreadID,
                    ) {
                        let mut creation = FILETIME::default();
                        let mut exit = FILETIME::default();
                        let mut kernel = FILETIME::default();
                        let mut user = FILETIME::default();

                        if GetThreadTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
                            .is_ok()
                        {
                            let kernel_ms = filetime_to_ms(&kernel);
                            let user_ms = filetime_to_ms(&user);

                            // Best-effort name lookup. Threads without a
                            // SetThreadDescription call land here as empty
                            // and the formatter shows "-" instead.
                            let name = GetThreadDescription(handle)
                                .ok()
                                .and_then(|pwstr| {
                                    let s = pwstr.to_string().ok().unwrap_or_default();
                                    if s.is_empty() {
                                        None
                                    } else {
                                        Some(s)
                                    }
                                })
                                .unwrap_or_default();

                            threads.push(ThreadSample {
                                id: entry.th32ThreadID,
                                total_ms: kernel_ms + user_ms,
                                user_ms,
                                kernel_ms,
                                name,
                            });
                        }
                        let _ = CloseHandle(handle);
                    }
                }

                entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
                if Thread32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }

    threads
}

/// FILETIME is in 100-nanosecond intervals; convert to milliseconds.
fn filetime_to_ms(ft: &windows::Win32::Foundation::FILETIME) -> f64 {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
    ticks as f64 / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_to_ms_converts_100ns_ticks() {
        // 1 tick = 100ns = 0.0001 ms
        let ft = windows::Win32::Foundation::FILETIME {
            dwLowDateTime: 10_000,
            dwHighDateTime: 0,
        };
        assert!((filetime_to_ms(&ft) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn filetime_to_ms_handles_high_dword() {
        // (1 << 32) ticks = 4_294_967_296 * 100ns = 429_496.7296 ms
        let ft = windows::Win32::Foundation::FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 1,
        };
        let expected = 4_294_967_296.0_f64 / 10_000.0;
        assert!((filetime_to_ms(&ft) - expected).abs() < 1e-3);
    }
}

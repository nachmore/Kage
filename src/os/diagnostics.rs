//! Cross-platform diagnostics: thread CPU sampling for the dump-thread-info
//! debug command. The actual snapshot uses Toolhelp/`GetThreadTimes` on
//! Windows and Mach `task_threads`/`thread_info` on macOS — see the
//! per-platform impls under `os/<plat>/diagnostics.rs`. Linux has no
//! native impl yet (a placeholder returns `Unsupported`).
//!
//! The text-formatting layer (delta sort, "← HOT/SPINNING" annotations,
//! cumulative-CPU table) is implemented here so the per-OS impls only
//! provide raw samples.

/// One thread's CPU usage at a moment in time. `id` is a stable, OS-defined
/// thread identifier (Windows TID, macOS Mach port). `name` is best-effort
/// (Windows reads `GetThreadDescription`; macOS leaves it empty).
#[derive(Clone, Debug)]
pub struct ThreadSample {
    pub id: u32,
    pub total_ms: f64,
    pub user_ms: f64,
    pub kernel_ms: f64,
    pub name: String,
}

/// Sample CPU usage for every thread in the current process.
/// Empty on Linux today.
pub fn sample_threads() -> Vec<ThreadSample> {
    crate::os::platform::diagnostics::sample_threads_impl()
}

/// Whether the current platform implements thread sampling. The dump
/// command uses this to render a "not implemented" message instead of
/// an empty table on Linux.
pub fn supports_thread_sampling() -> bool {
    crate::os::platform::diagnostics::supports_thread_sampling_impl()
}

/// Decorate a per-core CPU percentage with a hot/spinning marker.
pub fn cpu_pct_note(pct: f64) -> &'static str {
    if pct > 80.0 {
        " ← SPINNING"
    } else if pct > 30.0 {
        " ← HOT"
    } else {
        ""
    }
}

/// Sample, sleep 3s, sample again, format the deltas as a human-readable
/// dump. Same for every platform — the only OS-specific bit is
/// `sample_threads` itself.
pub fn dump_thread_info() -> String {
    use std::fmt::Write;

    if !supports_thread_sampling() {
        return "Thread dump not implemented on this platform".to_string();
    }

    let pid = std::process::id();
    let mut output = String::new();
    let _ = writeln!(output, "=== Thread Dump for PID {} ===", pid);

    let snap1 = sample_threads();
    if snap1.is_empty() {
        let _ = writeln!(output, "Failed to snapshot threads");
        return output;
    }

    let _ = writeln!(output, "Sampling {} threads for 3 seconds...", snap1.len());
    std::thread::sleep(std::time::Duration::from_secs(3));

    let snap2 = sample_threads();

    // Compute deltas. Pairs by stable id (TID/Mach port) so threads that
    // appeared or disappeared between snapshots are dropped — they can't
    // produce a meaningful delta anyway.
    // (id, dt, du, dk, ct, cu, ck, name)
    type Delta = (u32, f64, f64, f64, f64, f64, f64, String);
    let mut deltas: Vec<Delta> = Vec::new();
    for s2 in &snap2 {
        if let Some(s1) = snap1.iter().find(|s| s.id == s2.id) {
            let dt = s2.total_ms - s1.total_ms;
            let du = s2.user_ms - s1.user_ms;
            let dk = s2.kernel_ms - s1.kernel_ms;
            deltas.push((
                s2.id,
                dt,
                du,
                dk,
                s2.total_ms,
                s2.user_ms,
                s2.kernel_ms,
                s2.name.clone(),
            ));
        }
    }

    // Sort by delta descending so the noisiest thread comes first.
    deltas.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Active threads (used >10ms in the 3s window — anything quieter is
    // background noise we can elide for readability).
    let active: Vec<_> = deltas.iter().filter(|d| d.1 > 10.0).collect();
    if active.is_empty() {
        let _ = writeln!(
            output,
            "No threads used significant CPU in the 3s sample window."
        );
    } else {
        let _ = writeln!(output, "\n--- Active threads (CPU used in last 3s) ---");
        let _ = writeln!(
            output,
            "{:<8} {:<22} {:>10} {:>10} {:>10}  {:>12} {:>12} {:>12}",
            "TID", "Name", "Δ Total", "Δ User", "Δ Kernel", "Cum Total", "Cum User", "Cum Kernel"
        );
        let _ = writeln!(output, "{}", "-".repeat(105));
        for (tid, dt, du, dk, ct, cu, ck, name) in &active {
            let pct = dt / 3000.0 * 100.0; // % of one core
            let note = cpu_pct_note(pct);
            let display_name = if name.is_empty() { "-" } else { name.as_str() };
            let _ = writeln!(output, "{:<8} {:<22} {:>9.0}ms {:>9.0}ms {:>9.0}ms  {:>11.0}ms {:>11.0}ms {:>11.0}ms  ({:.0}% core){}",
                tid, display_name, dt, du, dk, ct, cu, ck, pct, note);
        }
    }

    // Top 10 by cumulative total — useful when a thread has been hot
    // since process start but isn't accumulating much in the 3s window.
    let _ = writeln!(output, "\n--- All threads by cumulative CPU (top 10) ---");
    let _ = writeln!(
        output,
        "{:<8} {:<22} {:>12} {:>12} {:>12}",
        "TID", "Name", "Total(ms)", "User(ms)", "Kernel(ms)"
    );
    let _ = writeln!(output, "{}", "-".repeat(72));
    let mut by_cum = deltas.clone();
    by_cum.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    for (tid, _, _, _, ct, cu, ck, name) in by_cum.iter().take(10) {
        let display_name = if name.is_empty() { "-" } else { name.as_str() };
        let _ = writeln!(
            output,
            "{:<8} {:<22} {:>12.0} {:>12.0} {:>12.0}",
            tid, display_name, ct, cu, ck
        );
    }

    let _ = writeln!(output, "\n=== End Thread Dump ===");

    // Mirror to the app log so an end-user can hit the tray menu and copy
    // the resulting log file to a bug report without retyping the dump.
    for line in output.lines() {
        log::info!("[ThreadDump] {}", line);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_pct_note_thresholds() {
        // SPINNING fires at >80, HOT at >30 (strict greater).
        assert_eq!(cpu_pct_note(0.0), "");
        assert_eq!(cpu_pct_note(30.0), ""); // boundary not inclusive
        assert_eq!(cpu_pct_note(30.1), " ← HOT");
        assert_eq!(cpu_pct_note(80.0), " ← HOT"); // boundary not inclusive
        assert_eq!(cpu_pct_note(80.1), " ← SPINNING");
        assert_eq!(cpu_pct_note(100.0), " ← SPINNING");
        // Negative shouldn't happen but mustn't panic
        assert_eq!(cpu_pct_note(-1.0), "");
    }

    #[test]
    fn dump_thread_info_returns_useful_output_on_supported_platforms() {
        // Skip the slow path (3s sleep) on platforms that don't sample.
        if !supports_thread_sampling() {
            assert_eq!(
                dump_thread_info(),
                "Thread dump not implemented on this platform"
            );
            return;
        }
        let snap = sample_threads();
        assert!(
            !snap.is_empty(),
            "every supported platform should report at least one thread"
        );
        // Every sample's components must satisfy total = user + kernel
        // (within float tolerance) — the platform impls construct it
        // that way and downstream callers depend on it.
        for s in &snap {
            let diff = (s.total_ms - (s.user_ms + s.kernel_ms)).abs();
            assert!(
                diff < 0.5,
                "total_ms ({}) should equal user+kernel ({}) for tid {}",
                s.total_ms,
                s.user_ms + s.kernel_ms,
                s.id
            );
        }
    }
}

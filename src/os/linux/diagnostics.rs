//! Thread sampling stub for Linux. `/proc/self/task/<tid>/stat` would
//! work but isn't wired up — the dump-thread-info debug tooling has
//! not yet shown a need for it on Linux. The cross-platform layer
//! detects this via `supports_thread_sampling_impl == false` and
//! prints "not implemented on this platform" instead of an empty
//! table.

use crate::os::diagnostics::ThreadSample;

pub fn supports_thread_sampling_impl() -> bool {
    false
}

pub fn sample_threads_impl() -> Vec<ThreadSample> {
    Vec::new()
}

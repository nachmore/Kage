// Linux side of `os::kage_target`. Input synthesis isn't implemented on
// Linux (os/linux/input.rs), so nothing here can be the target of one;
// only the process check is real.

pub fn is_kage_process_impl(pid: u32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .is_some_and(|p| crate::os::kage_target::is_kage_executable(&p))
}

pub fn keyboard_target_pids_impl() -> Vec<u32> {
    Vec::new()
}

pub fn window_owners_at_impl(_x: i32, _y: i32) -> Vec<u32> {
    Vec::new()
}

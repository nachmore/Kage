// Code shared by the macOS and Linux implementations (kage-core subset).
// The platform modules re-export what they use from here, so callers still
// go through `crate::os::platform::<mod>`.

pub mod host_bridge;

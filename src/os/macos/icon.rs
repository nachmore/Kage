// macOS icon extraction (stub — not yet implemented)
//
// A real implementation would use NSWorkspace.iconForFile + NSImage TIFF
// representation through the `objc` crate or `cocoa-foundation`. Until
// that's wired up, return None and warn once so the empty results aren't
// mysterious.

use std::sync::OnceLock;

static WARNED: OnceLock<()> = OnceLock::new();

fn warn_once() {
    WARNED.get_or_init(|| {
        log::warn!(
            "icon: macOS implementation not yet available — \
             returning no icon. NSWorkspace integration is a follow-up."
        );
    });
}

pub fn extract_icon_base64_impl(_path: &str) -> Option<String> {
    warn_once();
    None
}

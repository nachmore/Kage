// Linux icon extraction (stub — not yet implemented)
//
// A real implementation would resolve via the freedesktop icon theme spec:
// look up `icon-name` from the .desktop file, walk `~/.local/share/icons/`,
// `/usr/share/icons/<theme>/`, and `/usr/share/pixmaps/`, prefer SVG/PNG
// at common sizes, fall back to the hicolor theme. Until that's wired up,
// return None and warn once so the empty results aren't mysterious.

use std::sync::OnceLock;

static WARNED: OnceLock<()> = OnceLock::new();

fn warn_once() {
    WARNED.get_or_init(|| {
        log::warn!(
            "icon: Linux implementation not yet available — \
             returning no icon. Freedesktop icon theme lookup is a follow-up."
        );
    });
}

pub fn extract_icon_base64_impl(_path: &str) -> Option<String> {
    warn_once();
    None
}

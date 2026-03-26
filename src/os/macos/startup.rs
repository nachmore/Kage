// macOS startup management via LaunchAgent plist

use log::{info, warn};
use std::fs;
use std::path::PathBuf;

const PLIST_LABEL: &str = "com.kage.app";

fn get_plist_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/LaunchAgents").join(format!("{}.plist", PLIST_LABEL)))
}

pub fn get_startup_enabled_impl() -> bool {
    get_plist_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn set_startup_enabled_impl(enabled: bool) {
    let plist_path = match get_plist_path() {
        Some(p) => p,
        None => { warn!("Could not determine LaunchAgents path"); return; }
    };

    if enabled {
        let exe = std::env::current_exe().unwrap_or_default();
        let plist_content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>"#,
            PLIST_LABEL,
            exe.to_string_lossy()
        );

        if let Some(parent) = plist_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::write(&plist_path, plist_content) {
            Ok(_) => info!("Startup LaunchAgent plist created: {:?}", plist_path),
            Err(e) => warn!("Failed to create LaunchAgent plist: {}", e),
        }
    } else {
        match fs::remove_file(&plist_path) {
            Ok(_) => info!("Startup LaunchAgent plist removed"),
            Err(e) => warn!("Failed to remove LaunchAgent plist: {}", e),
        }
    }
}

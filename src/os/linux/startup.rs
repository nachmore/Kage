// Linux startup management via XDG autostart .desktop file

use log::{info, warn};
use std::fs;
use std::path::PathBuf;

const DESKTOP_FILENAME: &str = "kage.desktop";

fn get_autostart_path() -> Option<PathBuf> {
    dirs::config_dir().map(|c| c.join("autostart").join(DESKTOP_FILENAME))
}

pub fn get_startup_enabled_impl() -> bool {
    get_autostart_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn set_startup_enabled_impl(enabled: bool) {
    let desktop_path = match get_autostart_path() {
        Some(p) => p,
        None => {
            warn!("Could not determine autostart path");
            return;
        }
    };

    if enabled {
        let exe = std::env::current_exe().unwrap_or_default();
        let desktop_content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Kage\n\
             Exec={}\n\
             X-GNOME-Autostart-enabled=true\n\
             Hidden=false\n",
            exe.to_string_lossy()
        );

        if let Some(parent) = desktop_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::write(&desktop_path, desktop_content) {
            Ok(_) => info!("Startup .desktop file created: {:?}", desktop_path),
            Err(e) => warn!("Failed to create .desktop file: {}", e),
        }
    } else {
        match fs::remove_file(&desktop_path) {
            Ok(_) => info!("Startup .desktop file removed"),
            Err(e) => warn!("Failed to remove .desktop file: {}", e),
        }
    }
}

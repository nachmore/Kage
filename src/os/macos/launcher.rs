// macOS application launcher

use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::os::launcher::AppInfo;

pub fn scan_applications_impl() -> Result<Vec<AppInfo>> {
    let mut apps = Vec::new();
    
    let applications_dir = PathBuf::from("/Applications");
    if applications_dir.exists() {
        if let Ok(entries) = fs::read_dir(&applications_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("app") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        let icon_path = path.to_string_lossy().to_string();
                        apps.push(AppInfo {
                            name: name.to_string(),
                            path: path.clone(),
                            icon_path: Some(icon_path),
                        });
                    }
                }
            }
        }
    }
    
    Ok(apps)
}

pub fn launch_application_impl(path: &PathBuf) -> Result<()> {
    info!("Launching macOS application at {:?}", path);
    Command::new("open")
        .arg(path)
        .spawn()
        .context("Failed to launch application")?;
    Ok(())
}

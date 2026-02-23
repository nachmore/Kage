// Linux application launcher

use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::os::launcher::AppInfo;

pub fn scan_applications_impl() -> Result<Vec<AppInfo>> {
    let mut apps = Vec::new();
    
    // Scan .desktop files in standard locations
    let mut desktop_dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];
    
    if let Some(home) = dirs::home_dir() {
        desktop_dirs.push(home.join(".local/share/applications"));
    }
    
    for dir in desktop_dirs {
        if dir.exists() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("desktop") {
                        if let Ok(content) = fs::read_to_string(&path) {
                            if let Some(app_info) = parse_desktop_file(&content, &path) {
                                apps.push(app_info);
                            }
                        }
                    }
                }
            }
        }
    }
    
    Ok(apps)
}

fn parse_desktop_file(content: &str, path: &PathBuf) -> Option<AppInfo> {
    let mut name = None;
    let mut exec = None;
    
    for line in content.lines() {
        if line.starts_with("Name=") {
            name = Some(line[5..].to_string());
        } else if line.starts_with("Exec=") {
            exec = Some(line[5..].to_string());
        }
    }
    
    if let (Some(name), Some(exec)) = (name, exec) {
        Some(AppInfo {
            name,
            path: PathBuf::from(&exec),
            icon_path: Some(exec),
            emoji_icon: None,
            icon_data: None,
        })
    } else {
        None
    }
}

pub fn launch_application_impl(path: &PathBuf) -> Result<()> {
    info!("Launching Linux application at {:?}", path);
    
    if path.extension().and_then(|s| s.to_str()) == Some("desktop") {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .context("Failed to launch application")?;
    } else {
        Command::new(path)
            .spawn()
            .context("Failed to launch application")?;
    }
    
    Ok(())
}

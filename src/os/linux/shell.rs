// Linux shell operations

use anyhow::{Context, Result};
use std::process::Command;

pub fn open_url_impl(url: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .context("Failed to open URL")?;
    Ok(())
}

pub fn open_path_impl(path: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(path)
        .spawn()
        .context("Failed to open path")?;
    Ok(())
}

/// Reveal a file in the default file manager.
/// Linux doesn't have a universal "select file" command, so we open the parent directory.
pub fn reveal_in_file_manager_impl(path: &str) -> Result<()> {
    let parent = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    Command::new("xdg-open")
        .arg(&parent)
        .spawn()
        .context("Failed to open file manager")?;
    Ok(())
}

/// Open a file in the default editor
pub fn open_in_editor_impl(path: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(path)
        .spawn()
        .context("Failed to open in editor")?;
    Ok(())
}

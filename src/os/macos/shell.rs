// macOS shell operations

use anyhow::{Context, Result};
use std::process::Command;

pub fn open_url_impl(url: &str) -> Result<()> {
    Command::new("open")
        .arg(url)
        .spawn()
        .context("Failed to open URL")?;
    Ok(())
}

pub fn open_path_impl(path: &str) -> Result<()> {
    Command::new("open")
        .arg(path)
        .spawn()
        .context("Failed to open path")?;
    Ok(())
}

/// Reveal a file in Finder, selecting it
pub fn reveal_in_file_manager_impl(path: &str) -> Result<()> {
    Command::new("open")
        .args(["-R", path])
        .spawn()
        .context("Failed to reveal in Finder")?;
    Ok(())
}

/// Open a file in the default editor
pub fn open_in_editor_impl(path: &str) -> Result<()> {
    Command::new("open")
        .arg(path)
        .spawn()
        .context("Failed to open in editor")?;
    Ok(())
}

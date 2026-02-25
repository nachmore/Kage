// Windows shell operations

use anyhow::{Context, Result};
use std::process::Command;

pub fn open_url_impl(url: &str) -> Result<()> {
    Command::new("cmd")
        .args(&["/C", "start", url])
        .spawn()
        .context("Failed to open URL")?;
    Ok(())
}

pub fn open_path_impl(path: &str) -> Result<()> {
    Command::new("explorer")
        .arg(path)
        .spawn()
        .context("Failed to open path")?;
    Ok(())
}

/// Reveal a file in Explorer, selecting it
pub fn reveal_in_file_manager_impl(path: &str) -> Result<()> {
    Command::new("explorer")
        .args(["/select,", path])
        .spawn()
        .context("Failed to reveal in Explorer")?;
    Ok(())
}

/// Open a file in the default editor
pub fn open_in_editor_impl(path: &str) -> Result<()> {
    Command::new("cmd")
        .args(["/C", "start", "", path])
        .spawn()
        .context("Failed to open in editor")?;
    Ok(())
}

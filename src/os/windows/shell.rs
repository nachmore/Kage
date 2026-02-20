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

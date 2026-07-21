use log::{info, warn};

fn marker(name: &str) -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|dir| dir.join("kage").join(name))
}

fn write_marker(name: &str, value: &str, description: &str) {
    let Some(path) = marker(name) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            warn!("Failed to create updater marker directory: {error}");
            return;
        }
    }
    match std::fs::write(&path, value) {
        Ok(()) => info!("Wrote {description} marker to {path:?}"),
        Err(error) => warn!("Failed to write {description} marker: {error}"),
    }
}

/// Persist the active session so the restarted process can restore it.
pub fn persist_resume_marker(session_id: Option<&str>) {
    if let Some(session_id) = session_id {
        write_marker("last-session.txt", session_id, "resume");
    }
}

/// Source of an install, used to decide the post-restart UI behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallSource {
    Interactive,
    Idle,
}

impl InstallSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Idle => "idle",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "interactive" => Some(Self::Interactive),
            "idle" => Some(Self::Idle),
            _ => None,
        }
    }
}

pub fn persist_install_source(source: InstallSource) {
    write_marker("install-source.txt", source.as_str(), "install source");
}

/// Consume the install source marker, deleting it even if its content is invalid.
pub fn consume_install_source() -> Option<InstallSource> {
    let path = marker("install-source.txt")?;
    let contents = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(path);
    contents.as_deref().and_then(InstallSource::parse)
}

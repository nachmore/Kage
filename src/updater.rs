//! Signed application update orchestration.
//!
//! This facade keeps the updater's public API stable while its independent
//! concerns live in focused modules.

mod changelog;
mod checks;
mod install;
mod markers;
mod schedule;
mod state;

pub use changelog::fetch_changelog;
pub use checks::{
    endpoint_for_channel, plugin_check, CHANGELOG_URL, CURRENT_VERSION, ENDPOINT_BETA,
    ENDPOINT_DEV, ENDPOINT_STABLE, PUBKEY,
};
pub use install::{classify_install_error, plugin_download_and_install, relaunch_and_exit};
pub use markers::{
    consume_install_source, persist_install_source, persist_resume_marker, InstallSource,
};
pub use schedule::start_update_loop;
pub use state::UpdaterState;

use crate::config::Config;

/// Check whether the running build is the version recorded before installation.
pub fn was_just_updated(config: &Config) -> bool {
    config
        .updates
        .last_updated_version
        .as_ref()
        .is_some_and(|version| version == CURRENT_VERSION)
}

/// Clear the post-update notification marker after it has been shown.
pub fn clear_update_flag(config: &mut Config) {
    config.updates.last_updated_version = None;
}

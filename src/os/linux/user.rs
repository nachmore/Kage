// Linux user avatar lookup

/// Find the user's profile picture on Linux
pub fn get_avatar_path_impl(username: &str) -> Option<String> {
    // freedesktop: ~/.face
    if let Some(home) = dirs::home_dir() {
        let face = home.join(".face");
        if face.exists() {
            return face.to_str().map(|s| s.to_string());
        }
    }

    // AccountsService icon
    let accounts_icon = std::path::PathBuf::from("/var/lib/AccountsService/icons").join(username);
    if accounts_icon.exists() {
        return accounts_icon.to_str().map(|s| s.to_string());
    }

    None
}

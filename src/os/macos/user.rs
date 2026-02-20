// macOS user avatar lookup

/// Find the user's profile picture on macOS
pub fn get_avatar_path_impl(_username: &str) -> Option<String> {
    let home = dirs::home_dir()?;

    // Check ~/.face (freedesktop convention some macOS tools use)
    let face = home.join(".face");
    if face.exists() {
        return face.to_str().map(|s| s.to_string());
    }

    None
}

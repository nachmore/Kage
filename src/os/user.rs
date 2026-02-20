// Cross-platform user information

/// User profile information
pub struct UserProfile {
    pub display_name: String,
    pub username: String,
    pub avatar_path: Option<String>,
}

/// Get the current user's profile information
pub fn get_user_profile() -> UserProfile {
    let username = whoami::username();
    let display_name = whoami::realname();

    let avatar_path = get_avatar_path_impl(&username);

    UserProfile {
        display_name: if display_name.is_empty() {
            username.clone()
        } else {
            display_name
        },
        username,
        avatar_path,
    }
}

#[cfg(target_os = "windows")]
fn get_avatar_path_impl(username: &str) -> Option<String> {
    crate::os::windows::user::get_avatar_path_impl(username)
}

#[cfg(target_os = "macos")]
fn get_avatar_path_impl(username: &str) -> Option<String> {
    crate::os::macos::user::get_avatar_path_impl(username)
}

#[cfg(target_os = "linux")]
fn get_avatar_path_impl(username: &str) -> Option<String> {
    crate::os::linux::user::get_avatar_path_impl(username)
}

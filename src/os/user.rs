// Cross-platform user information

/// User profile information
pub struct UserProfile {
    pub display_name: String,
    pub username: String,
    pub avatar_path: Option<String>,
}

/// Get the current user's profile information
pub fn get_user_profile() -> UserProfile {
    let username = whoami::username().unwrap_or_else(|_| "user".to_string());
    let display_name = whoami::realname().unwrap_or_else(|_| String::new());
    let avatar_path = crate::os::platform::user::get_avatar_path_impl(&username);

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

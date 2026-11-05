// Cross-platform regional preferences that the locale tag doesn't carry.
//
// `sys_locale::get_locale()` gives a BCP-47 tag such as `en-US`, but some
// regional settings are separate user preferences the tag says nothing
// about — most visibly the first day of the week (macOS: System Settings →
// General → Language & Region → First day of week; Windows: Region →
// "First day of week"). An en-US user who picked Monday there expects
// Monday-first calendars.

/// The user's first day of the week, as a JavaScript weekday (0 = Sunday …
/// 6 = Saturday), or `None` when the OS doesn't expose one (Linux). The
/// frontend falls back to the locale's CLDR default when this is `None`.
pub fn first_weekday() -> Option<u8> {
    crate::os::platform::locale::first_weekday_impl()
}

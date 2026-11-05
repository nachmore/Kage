// Linux: no portable first-day-of-week preference (glibc's
// `_NL_TIME_FIRST_WEEKDAY` follows LC_TIME, which the locale tag already
// implies), so the frontend uses the locale's CLDR default.

pub fn first_weekday_impl() -> Option<u8> {
    None
}

// Windows regional preferences via the NLS locale API.

#[link(name = "kernel32")]
extern "system" {
    fn GetLocaleInfoEx(locale_name: *const u16, lc_type: u32, data: *mut u16, cch_data: i32)
        -> i32;
}

/// `LOCALE_IFIRSTDAYOFWEEK` (winnls.h).
const LOCALE_IFIRSTDAYOFWEEK: u32 = 0x0000_100C;

/// The user-default locale's first day of the week — this reflects a
/// "First day of week" override in Settings → Time & language → Region.
pub fn first_weekday_impl() -> Option<u8> {
    let mut buf = [0u16; 8];
    // SAFETY: a NULL locale name is LOCALE_NAME_USER_DEFAULT; `buf` is a
    // valid, writable buffer of the length we pass.
    let written = unsafe {
        GetLocaleInfoEx(
            std::ptr::null(),
            LOCALE_IFIRSTDAYOFWEEK,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if written <= 0 {
        return None;
    }
    // `written` counts the terminating NUL.
    let len = (written as usize).saturating_sub(1).min(buf.len());
    let value = String::from_utf16_lossy(&buf[..len]);
    weekday_from_nls(value.trim().parse().ok()?)
}

/// NLS numbers weekdays 0 = Monday … 6 = Sunday.
fn weekday_from_nls(n: u32) -> Option<u8> {
    (n <= 6).then(|| ((n + 1) % 7) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nls_weekdays_map_to_js_weekdays() {
        assert_eq!(weekday_from_nls(0), Some(1)); // Monday
        assert_eq!(weekday_from_nls(5), Some(6)); // Saturday
        assert_eq!(weekday_from_nls(6), Some(0)); // Sunday
        assert_eq!(weekday_from_nls(7), None);
    }

    #[test]
    fn reads_a_weekday_from_the_os() {
        assert!(matches!(first_weekday_impl(), Some(0..=6)));
    }
}

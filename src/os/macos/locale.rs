// macOS regional preferences via CoreFoundation.

type CFTypeRef = *const std::ffi::c_void;

extern "C" {
    fn CFCalendarCopyCurrent() -> CFTypeRef;
    fn CFCalendarGetFirstWeekday(calendar: CFTypeRef) -> isize;
    fn CFRelease(cf: CFTypeRef);
}

/// `CFCalendarCopyCurrent` is the user's calendar with their Language &
/// Region settings applied, including an explicit "First day of week"
/// (`AppleFirstWeekday`) that differs from the region's default.
pub fn first_weekday_impl() -> Option<u8> {
    // SAFETY: CFCalendarCopyCurrent returns a +1 retained CFCalendarRef (or
    // NULL); we read one scalar from it and release it exactly once.
    let raw = unsafe {
        let cal = CFCalendarCopyCurrent();
        if cal.is_null() {
            return None;
        }
        let n = CFCalendarGetFirstWeekday(cal);
        CFRelease(cal);
        n
    };
    weekday_from_cf(raw)
}

/// CFCalendar numbers weekdays 1 = Sunday … 7 = Saturday.
fn weekday_from_cf(n: isize) -> Option<u8> {
    (1..=7).contains(&n).then(|| (n - 1) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_weekdays_map_to_js_weekdays() {
        assert_eq!(weekday_from_cf(1), Some(0)); // Sunday
        assert_eq!(weekday_from_cf(2), Some(1)); // Monday
        assert_eq!(weekday_from_cf(7), Some(6)); // Saturday
        assert_eq!(weekday_from_cf(0), None);
        assert_eq!(weekday_from_cf(8), None);
    }

    #[test]
    fn reads_a_weekday_from_the_os() {
        assert!(matches!(first_weekday_impl(), Some(0..=6)));
    }
}

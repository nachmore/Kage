// macOS clipboard operations

use log::info;
use std::process::Command;

/// `kVK_ANSI_C` / `kVK_ANSI_V` from <HIToolbox/Events.h>. Not exposed by
/// any of our crates so we hard-code them. Stable across macOS releases.
const KEYCODE_C: u16 = 0x08;
const KEYCODE_V: u16 = 0x09;

/// How long `finish` waits for the target app to answer the synthetic
/// Cmd+C before concluding nothing was selected. Matches Windows.
const COPY_TIMEOUT_MS: u64 = 300;

/// Two-phase capture, mirroring the Windows design: `begin` snapshots the
/// clipboard + `NSPasteboard.changeCount` and posts Cmd+C (no process
/// spawns, ~1ms, so it's safe on the hotkey callback thread); `finish`
/// polls changeCount so it returns as soon as the copy lands instead of
/// sleeping blind.
pub struct SelectionCaptureToken {
    original_clipboard: Option<String>,
    change_count_before: Option<isize>,
}

pub fn begin_selection_capture_impl() -> SelectionCaptureToken {
    let original_clipboard = read_clipboard_impl();
    let change_count_before = pasteboard::change_count();
    if let Err(reason) = post_command_keystroke(KEYCODE_C) {
        info!("[selection] Cmd+C synthesis failed: {reason}");
    }
    SelectionCaptureToken {
        original_clipboard,
        change_count_before,
    }
}

pub fn finish_selection_capture_impl(token: SelectionCaptureToken) -> Option<String> {
    let SelectionCaptureToken {
        original_clipboard,
        change_count_before,
    } = token;
    // No changeCount means NSPasteboard was unreachable — we can't tell a
    // fresh copy from stale contents, so don't guess.
    let before = change_count_before?;
    if !wait_for_pasteboard_change(before, COPY_TIMEOUT_MS) {
        return None;
    }
    // changeCount bumps on clearContents, before the app has finished
    // writing its types — give it a beat, like the Windows path does.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let new_text = read_clipboard_impl();
    // Only text can be put back through this API. A non-text original
    // (None — e.g. a copied image) is left alone rather than clobbered
    // with an empty string; the captured selection stays in its place.
    if let Some(orig) = &original_clipboard {
        write_clipboard_impl(orig);
    }
    // changeCount moved, so a copy happened — return the text even when it
    // equals the previous clipboard (user re-selected the same text).
    let captured = non_empty_trimmed(new_text)?;
    info!("[selection] Captured {} chars", captured.len());
    Some(captured)
}

fn non_empty_trimmed(text: Option<String>) -> Option<String> {
    let text = text?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn wait_for_pasteboard_change(before: isize, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if pasteboard::change_count().is_some_and(|c| c != before) {
            return true;
        }
    }
    false
}

/// Thin NSPasteboard access via the raw objc runtime (the objc2-app-kit
/// `NSPasteboard` feature isn't enabled). Reading through the pasteboard
/// rather than `pbpaste` gives us `None` for non-text contents (pbpaste
/// prints "" for an image, which the capture path would then "restore"
/// over the image) and is locale-independent.
mod pasteboard {
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;
    use std::ptr;

    fn general() -> Option<Retained<AnyObject>> {
        let cls = AnyClass::get(c"NSPasteboard")?;
        // SAFETY: +[NSPasteboard generalPasteboard] takes no arguments and
        // returns an NSPasteboard instance.
        unsafe { msg_send![cls, generalPasteboard] }
    }

    pub fn change_count() -> Option<isize> {
        autoreleasepool(|_| {
            let pb = general()?;
            // SAFETY: -changeCount returns NSInteger.
            let count: isize = unsafe { msg_send![&*pb, changeCount] };
            Some(count)
        })
    }

    /// Plain text if present; otherwise the text of an RTF- or HTML-only
    /// pasteboard (some apps publish only rich types, which pbpaste used
    /// to coerce for us). A rich payload with no readable text (e.g. the
    /// `<img>` wrapper a browser adds to a copied image) stays `None`, so
    /// the capture path won't "restore" an empty string over the image.
    pub fn read_text() -> Option<String> {
        autoreleasepool(|_| {
            let pb = general()?;
            if let Some(text) = string_for_type(&pb, "public.utf8-plain-text") {
                return Some(text);
            }
            let readable = |text: &String| !text.trim().is_empty();
            rtf_text(&pb).filter(readable).or_else(|| {
                string_for_type(&pb, "public.html")
                    .map(|html| super::html_to_text(&html))
                    .filter(readable)
            })
        })
    }

    fn string_for_type(pb: &AnyObject, ty: &str) -> Option<String> {
        let ty = NSString::from_str(ty);
        // SAFETY: -stringForType: takes an NSPasteboardType (NSString)
        // and returns a nullable NSString.
        let text: Option<Retained<NSString>> = unsafe { msg_send![pb, stringForType: &*ty] };
        text.map(|s| s.to_string())
    }

    /// Flatten `public.rtf` via NSAttributedString's AppKit RTF importer.
    /// Unlike the HTML importer (WebKit-backed, main-thread only) the RTF
    /// reader is safe off the main thread, which matters because
    /// `read_clipboard` runs on async command workers.
    fn rtf_text(pb: &AnyObject) -> Option<String> {
        let ty = NSString::from_str("public.rtf");
        // SAFETY: -dataForType: takes an NSPasteboardType (NSString) and
        // returns a nullable NSData.
        let data: Option<Retained<AnyObject>> = unsafe { msg_send![pb, dataForType: &*ty] };
        let data = data?;
        let cls = AnyClass::get(c"NSAttributedString")?;
        // SAFETY: +alloc returns an uninitialised +1 instance, consumed by
        // the init call below (which releases it itself if parsing fails).
        let alloc: Allocated<AnyObject> = unsafe { msg_send![cls, alloc] };
        // SAFETY: -initWithRTF:documentAttributes: takes NSData plus a
        // nullable `NSDictionary **` out-parameter (NULL = we don't want
        // the document attributes) and returns nil on malformed RTF.
        let attributed: Option<Retained<AnyObject>> = unsafe {
            msg_send![
                alloc,
                initWithRTF: &*data,
                documentAttributes: ptr::null_mut::<*mut AnyObject>()
            ]
        };
        let attributed = attributed?;
        // SAFETY: -[NSAttributedString string] returns a non-null NSString.
        let text: Retained<NSString> = unsafe { msg_send![&*attributed, string] };
        // Embedded pictures surface as U+FFFC attachment placeholders,
        // which carry no text — drop them so an image-only RTF stays None.
        Some(text.to_string().replace('\u{FFFC}', ""))
    }
}

/// Conservative HTML -> plain text for an HTML-only pasteboard. We don't
/// use `-[NSAttributedString initWithHTML:]`: that importer drives WebKit,
/// must run on the main thread and spins the run loop, while
/// `read_clipboard` is called from async command workers. We only need
/// the readable text, so: drop tags, comments and script/style/title
/// bodies, turn block boundaries into newlines, collapse whitespace
/// outside `<pre>`, and decode the common entities.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    // Tag name whose body is being discarded (script/style/...).
    let mut skipping: Option<String> = None;
    let mut pre_depth = 0usize;
    loop {
        let Some(lt) = rest.find('<') else {
            if skipping.is_none() {
                push_html_text(&mut out, rest, pre_depth > 0);
            }
            break;
        };
        let after = &rest[lt + 1..];
        let starts_markup =
            after.starts_with(|c: char| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        if !starts_markup {
            // A stray `<` (sloppy "a < b") is text, not markup.
            if skipping.is_none() {
                push_html_text(&mut out, &rest[..=lt], pre_depth > 0);
            }
            rest = after;
            continue;
        }
        if skipping.is_none() {
            push_html_text(&mut out, &rest[..lt], pre_depth > 0);
        }
        if let Some(comment) = after.strip_prefix("!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        // Unterminated tag: nothing readable follows it.
        let Some(gt) = find_tag_end(after) else {
            break;
        };
        let tag = &after[..gt];
        rest = &after[gt + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if let Some(end) = &skipping {
            if closing && name == *end {
                skipping = None;
            }
            continue;
        }
        if !closing && matches!(name.as_str(), "script" | "style" | "title" | "template") {
            skipping = Some(name);
            continue;
        }
        match name.as_str() {
            "br" => {
                trim_trailing_blanks(&mut out);
                out.push('\n');
            }
            "pre" => {
                if closing {
                    pre_depth = pre_depth.saturating_sub(1);
                } else {
                    pre_depth += 1;
                }
                start_new_line(&mut out);
            }
            "td" | "th" if !closing => {
                // Cells on the same row read best tab-separated.
                if !out.is_empty() && !out.ends_with(['\n', '\t']) {
                    out.truncate(out.trim_end_matches(' ').len());
                    out.push('\t');
                }
            }
            "p" | "div" | "li" | "tr" | "ul" | "ol" | "table" | "blockquote" | "hr" | "h1"
            | "h2" | "h3" | "h4" | "h5" | "h6" | "dt" | "dd" | "section" | "article" | "header"
            | "footer" => start_new_line(&mut out),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Index of the `>` closing a tag, ignoring any inside quoted attribute
/// values (`title="a>b"`). Only a quote right after `=` opens a value, so
/// a stray apostrophe in an unquoted attribute can't swallow the rest of
/// the document.
fn find_tag_end(tag: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut prev_non_space = ' ';
    for (i, c) in tag.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '>' => return Some(i),
            None if matches!(c, '"' | '\'') && prev_non_space == '=' => quote = Some(c),
            None => {}
        }
        if !c.is_whitespace() {
            prev_non_space = c;
        }
    }
    None
}

fn push_html_text(out: &mut String, raw: &str, preformatted: bool) {
    let text = decode_html_entities(raw);
    if preformatted {
        out.push_str(&text);
        return;
    }
    // HTML renders any whitespace run as a single space.
    for c in text.chars() {
        if !c.is_whitespace() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with(char::is_whitespace) {
            out.push(' ');
        }
    }
}

fn trim_trailing_blanks(out: &mut String) {
    out.truncate(out.trim_end_matches([' ', '\t']).len());
}

fn start_new_line(out: &mut String) {
    trim_trailing_blanks(out);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn decode_html_entities(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('&') {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        // The entities we decode are short; a far-off `;` means this `&`
        // is literal text.
        let decoded = after
            .find(';')
            .filter(|&end| end <= 10)
            .and_then(|end| decode_html_entity(&after[..end]).map(|c| (c, end)));
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

fn decode_html_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        // A plain space: NBSPs in pasted text trip up search and diffs.
        "nbsp" => Some(' '),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse::<u32>().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Simulate a Cmd+V paste keystroke into the foreground window via
/// CGEvent. Requires Accessibility permission (macOS 10.15+); if the
/// permission isn't granted the CGEventPost calls silently no-op and
/// nothing is pasted. We still warn once so a missing permission is
/// visible in the logs.
pub fn simulate_paste_impl() {
    if let Err(reason) = post_command_keystroke(KEYCODE_V) {
        warn_paste_once(reason);
    }
}

/// Post a Cmd+<key> down/up pair via CGEvent. We modify only with the
/// Command flag — set explicitly so modifiers still physically held from
/// the hotkey (e.g. Shift) don't turn it into a different shortcut.
fn post_command_keystroke(keycode: u16) -> Result<(), &'static str> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // CombinedSessionState posts events as if they came from the user —
    // the same channel keystrokes normally travel on. HIDSystemState is
    // for lower-level synthetic input that bypasses session-level
    // modifications; we don't need that.
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|()| "CGEventSourceCreate returned NULL")?;

    let down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
        .map_err(|()| "CGEventCreateKeyboardEvent(keydown) returned NULL")?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);

    let up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|()| "CGEventCreateKeyboardEvent(keyup) returned NULL")?;
    up.set_flags(CGEventFlags::CGEventFlagCommand);

    // Post to the HID event tap — the earliest point in the input pipeline,
    // which means the target window sees the synthetic event identically
    // to a real keystroke. Without Accessibility permission these calls
    // complete successfully but the OS drops the events; there's no API
    // to detect that case at post time.
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

/// Log the paste-failure reason exactly once per process. We don't want
/// to spam the log on every hotkey when Accessibility permission is
/// missing — one line with the specific failure mode is enough.
fn warn_paste_once(reason: &str) {
    use std::sync::OnceLock;
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        log::warn!(
            "simulate_paste: failed to synthesize Cmd+V ({reason}) — \
             check Accessibility permission in System Settings → Privacy & Security"
        );
    });
}

/// Text on the clipboard (RTF/HTML-only contents flattened to plain text),
/// or `None` when it holds no text at all (image, file, …).
pub fn read_clipboard_impl() -> Option<String> {
    pasteboard::read_text()
}

pub fn write_clipboard_impl(text: &str) -> bool {
    use std::io::Write;
    // pbcopy picks its text encoding from LANG, which a Finder-launched
    // .app doesn't have — without it non-ASCII text gets mangled.
    let Ok(mut child) = Command::new("pbcopy")
        .env("LANG", "en_US.UTF-8")
        .stdin(std::process::Stdio::piped())
        .spawn()
    else {
        return false;
    };
    if let Some(ref mut stdin) = child.stdin {
        if stdin.write_all(text.as_bytes()).is_err() {
            let _ = child.wait();
            return false;
        }
    }
    // pbcopy commits the clipboard on a clean exit; a non-zero status means
    // the write didn't land.
    matches!(child.wait(), Ok(status) if status.success())
}

pub fn capture_selection_impl() -> Option<String> {
    finish_selection_capture_impl(begin_selection_capture_impl())
}

#[cfg(test)]
mod tests {
    use super::{html_to_text, non_empty_trimmed};

    #[test]
    fn non_empty_trimmed_drops_blank_and_missing_text() {
        assert_eq!(non_empty_trimmed(None), None);
        assert_eq!(non_empty_trimmed(Some("  \n".into())), None);
        assert_eq!(non_empty_trimmed(Some(" hi \n".into())), Some("hi".into()));
    }

    #[test]
    fn html_to_text_strips_markup_and_decodes_entities() {
        let html = "<meta charset='utf-8'><!-- StartFragment --><p>Tom &amp; <b>Jerry</b>\n  \
                    &lt;3&#33;&nbsp;&#x41;</p><p>Second<br>line</p>";
        assert_eq!(html_to_text(html), "Tom & Jerry <3! A\nSecond\nline");
    }

    #[test]
    fn html_to_text_drops_script_style_and_title_bodies() {
        let html = "<html><head><title>T</title><style>p{color:red}</style></head>\
                    <body><script>var a = '<p>';</script>Body</body></html>";
        assert_eq!(html_to_text(html), "Body");
    }

    #[test]
    fn html_to_text_keeps_stray_brackets_and_quoted_gt() {
        assert_eq!(html_to_text("a < b & c"), "a < b & c");
        assert_eq!(html_to_text("<a title=\"x>y\" href='z'>link</a>"), "link");
    }

    #[test]
    fn html_to_text_preserves_pre_and_separates_cells() {
        assert_eq!(html_to_text("<pre>a\n  b</pre>"), "a\n  b");
        assert_eq!(
            html_to_text("<table><tr><td>1</td><td>2</td></tr><tr><td>3</td></tr></table>"),
            "1\t2\n3"
        );
    }

    #[test]
    fn html_to_text_of_image_only_markup_is_empty() {
        assert_eq!(html_to_text("<img src=\"https://x/y.png\" alt=\"\">"), "");
    }
}

// macOS icon extraction via NSWorkspace.iconForFile.
//
// Flow: NSWorkspace.iconForFile(path) → NSImage → CGImage at ICON_POINTS
// → NSBitmapImageRep → PNG data → base64 data URI.
//
// NSWorkspace handles both `.app` bundles (reads Contents/Resources/*.icns
// via the Info.plist CFBundleIconFile key) and regular files (falls back to
// UTI-based generic icons). No permissions required.
//
// Size matters a lot here. An app icon's NSImage carries every
// representation in the .icns, up to 1024×1024@2x, and the old
// `TIFFRepresentation` → PNG path encoded the largest one: ~0.5 s and
// ~1 MB of base64 per app, so the 121-app launcher scan took a minute and
// pinned >100 MB of icons in memory. `CGImageForProposedRect` instead
// picks the representation closest to the requested size (64 pt → 128 px
// on a Retina screen): ~1 ms and ~12 KB per icon, which is still sharper
// than anything the UI draws (≤ 48 px).

use base64::Engine;
use objc2::rc::autoreleasepool;
use objc2::AllocAnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

/// Edge length, in points, of the icon we encode.
const ICON_POINTS: f64 = 64.0;

pub fn extract_icon_base64_impl(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }

    // NSWorkspace documents that `iconForFile:` is safe to call from any
    // thread on modern macOS — it doesn't mutate shared UI state. We still
    // wrap in an autorelease pool so the intermediate objects (NSImage,
    // CGImage, NSBitmapImageRep, NSData) don't accumulate across many calls.
    autoreleasepool(|_pool| {
        let ns_path = NSString::from_str(path);
        let workspace = NSWorkspace::sharedWorkspace();
        let image = workspace.iconForFile(&ns_path);

        let mut rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(ICON_POINTS, ICON_POINTS),
        );
        // Safety: `rect` is a valid, exclusively borrowed NSRect for the
        // duration of the call; no context or hints are passed.
        let cg_image =
            unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);

        // Empty properties dict — for PNG the only supported properties are
        // interlacing flags and fallback colour; defaults are fine for icons.
        let props = NSDictionary::new();
        // Safety: `properties` is an empty NSDictionary so there are no
        // wrong-typed values. PNG storage type is a plain enum, no lifetime
        // concerns. Returning Option<Retained<NSData>> is safe to bubble.
        let png_data = unsafe {
            bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &props)
        }?;

        // Copy into a Vec — NSData.to_vec() is the safe extension on
        // objc2_foundation::NSData. Avoids keeping the NSData retained
        // across the base64 encode.
        let bytes = png_data.to_vec();
        if bytes.is_empty() {
            return None;
        }

        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(format!("data:image/png;base64,{}", b64))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finder is on every macOS install. Its icon must come back as a PNG
    /// data URI small enough that a full launcher scan stays cheap — the
    /// old full-resolution path produced ~1 MB per app.
    #[test]
    fn app_icon_is_a_small_png() {
        let started = std::time::Instant::now();
        let uri = extract_icon_base64_impl("/System/Library/CoreServices/Finder.app")
            .expect("Finder has an icon");
        assert!(uri.starts_with("data:image/png;base64,"));
        assert!(uri.len() < 200_000, "icon data URI is {} bytes", uri.len());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn empty_path_has_no_icon() {
        assert_eq!(extract_icon_base64_impl(""), None);
    }
}

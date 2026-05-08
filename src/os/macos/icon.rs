// macOS icon extraction via NSWorkspace.iconForFile.
//
// Flow: NSWorkspace.iconForFile(path) → NSImage → TIFFRepresentation
// → NSBitmapImageRep → PNG data → base64 data URI.
//
// NSWorkspace handles both `.app` bundles (reads Contents/Resources/*.icns
// via the Info.plist CFBundleIconFile key) and regular files (falls back to
// UTI-based generic icons). No permissions required.
//
// The bitmap size we return is whatever the NSImage's best representation
// resolves to — typically 32x32 or 64x64 depending on the icon bundle.
// The cross-platform launcher caches these so per-extraction cost isn't
// critical, but we avoid forcing a resize step since the natural size is
// usually what we want.

use base64::Engine;
use objc2::rc::autoreleasepool;
use objc2::AllocAnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
use objc2_foundation::{NSDictionary, NSString};

pub fn extract_icon_base64_impl(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }

    // NSWorkspace documents that `iconForFile:` is safe to call from any
    // thread on modern macOS — it doesn't mutate shared UI state. We still
    // wrap in an autorelease pool so the intermediate objects (NSImage,
    // NSData, NSBitmapImageRep) don't accumulate across many calls.
    autoreleasepool(|_pool| {
        let ns_path = NSString::from_str(path);
        let workspace = NSWorkspace::sharedWorkspace();
        let image = workspace.iconForFile(&ns_path);

        // NSImage → TIFFRepresentation → NSBitmapImageRep. TIFF is the
        // "universal" intermediate on macOS — it's the only format every
        // NSImage can round-trip through without loss.
        let tiff_data = image.TIFFRepresentation()?;
        let bitmap = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff_data)?;

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
        // across the base64 encode (cheap: PNGs for app icons are a few KB).
        let bytes = png_data.to_vec();
        if bytes.is_empty() {
            return None;
        }

        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(format!("data:image/png;base64,{}", b64))
    })
}

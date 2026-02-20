# Icon Generation Guide

## Current Status

The system tray icon is currently using `ui/assets/ghost.png`. Tauri will attempt to convert this to the appropriate format for each platform, but for best results, you should generate proper icon files.

## Quick Fix (Current)

Updated `tauri.conf.json` to use the existing ghost.png:
- System tray: `ui/assets/ghost.png`
- App bundle: `ui/assets/ghost.png` + `icons/icon.ico`

This should show the Kiro ghost in the system tray instead of a blank icon.

## Proper Icon Generation (Recommended)

For production, generate proper icon files in all required formats:

### Required Files

1. **Windows**: `icon.ico` (multi-resolution ICO file)
   - Should contain: 16x16, 32x32, 48x48, 256x256
   
2. **macOS**: `icon.icns` (Apple Icon Image format)
   - Should contain multiple resolutions up to 1024x1024

3. **Linux**: PNG files
   - 32x32.png
   - 128x128.png
   - 128x128@2x.png (256x256)

### Tools to Generate Icons

#### Option 1: Online Tools (Easiest)
- **iConvert Icons**: https://iconverticons.com/online/
  - Upload ghost.png
  - Download ICO and ICNS files
  
- **CloudConvert**: https://cloudconvert.com/png-to-ico
  - Convert PNG to ICO with multiple sizes

#### Option 2: Command Line Tools

**For ICO (Windows)**:
```bash
# Using ImageMagick
magick convert ui/assets/ghost.png -define icon:auto-resize=256,128,96,64,48,32,16 icons/icon.ico
```

**For ICNS (macOS)**:
```bash
# Create iconset directory
mkdir ghost.iconset

# Generate all required sizes
sips -z 16 16     ui/assets/ghost.png --out ghost.iconset/icon_16x16.png
sips -z 32 32     ui/assets/ghost.png --out ghost.iconset/icon_16x16@2x.png
sips -z 32 32     ui/assets/ghost.png --out ghost.iconset/icon_32x32.png
sips -z 64 64     ui/assets/ghost.png --out ghost.iconset/icon_32x32@2x.png
sips -z 128 128   ui/assets/ghost.png --out ghost.iconset/icon_128x128.png
sips -z 256 256   ui/assets/ghost.png --out ghost.iconset/icon_128x128@2x.png
sips -z 256 256   ui/assets/ghost.png --out ghost.iconset/icon_256x256.png
sips -z 512 512   ui/assets/ghost.png --out ghost.iconset/icon_256x256@2x.png
sips -z 512 512   ui/assets/ghost.png --out ghost.iconset/icon_512x512.png
sips -z 1024 1024 ui/assets/ghost.png --out ghost.iconset/icon_512x512@2x.png

# Convert to ICNS
iconutil -c icns ghost.iconset -o icons/icon.icns
```

**For PNG (Linux)**:
```bash
# Using ImageMagick
magick convert ui/assets/ghost.png -resize 32x32 icons/32x32.png
magick convert ui/assets/ghost.png -resize 128x128 icons/128x128.png
magick convert ui/assets/ghost.png -resize 256x256 icons/128x128@2x.png
```

#### Option 3: Use Tauri Icon Tool

Tauri provides a built-in icon generator:

```bash
# Install tauri-cli if not already installed
cargo install tauri-cli

# Generate all icon formats from a single PNG
cargo tauri icon ui/assets/ghost.png
```

This will automatically generate all required icon files in the correct formats and place them in the `icons/` directory.

## Recommended Approach

**For development**: Current setup with ghost.png is fine.

**For production**: Run the Tauri icon generator:
```bash
cargo tauri icon ui/assets/ghost.png
```

This will create:
- `icons/icon.ico` (Windows)
- `icons/icon.icns` (macOS)
- `icons/32x32.png` (Linux)
- `icons/128x128.png` (Linux)
- `icons/128x128@2x.png` (Linux)
- `icons/icon.png` (fallback)

## Testing

After generating icons:

1. **Rebuild the app**:
   ```bash
   cargo build --release
   ```

2. **Check system tray**:
   - Run the app
   - Look at the system tray (Windows notification area, macOS menu bar, Linux system tray)
   - The Kiro ghost should appear instead of a blank icon

3. **Check app icon**:
   - Look at the taskbar/dock icon
   - Should show the Kiro ghost

## Notes

- System tray icons should be simple and recognizable at small sizes (16x16, 32x32)
- The ghost.png should work well as it's already a simple, clear design
- For best results on Windows, the ICO file should include multiple resolutions
- macOS prefers ICNS format for best quality at all sizes
- The `iconAsTemplate: true` setting in tauri.conf.json makes the icon adapt to the system theme on macOS

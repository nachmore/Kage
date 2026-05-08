# Icons

Application icons generated from `kage-icon-basic.svg` (app icon) and `kage-icon.svg` (NSIS installer images) with the teal outline (`#38B2AC`).

## Generated files

- `32x32.png` — small app icon
- `128x128.png` — standard app icon
- `128x128@2x.png` — retina app icon (256x256)
- `kage.ico` — Windows icon (16, 32, 48, 256)
- `kage.icns` — macOS icon (16, 32, 128, 256, 512 + @2x variants)
- `nsis-header.bmp` — NSIS installer header (300x114, light bg)
- `nsis-sidebar.bmp` — NSIS installer sidebar (164x314, dark bg)

## Regenerating

Requires Python with Pillow and Inkscape installed. Generating `kage.icns`
additionally requires `iconutil` (ships with Xcode CLI tools) and is only
produced when the script runs on macOS — on Windows/Linux the icns
generation step is skipped automatically.

```bash
pip install Pillow
python icons/generate-icons.py
```

Colors are pulled from the theme tokens in `ui/css/shared-kage-tokens.css`. Update the constants at the top of the script if the theme changes.

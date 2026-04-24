"""Generate all application icons from the mascot SVG.

Reads ui/assets/kage-icon.svg, applies the teal outline, and generates:
  - icons/32x32.png
  - icons/128x128.png
  - icons/128x128@2x.png  (256x256)
  - icons/icon.ico          (multi-size Windows icon)
  - icons/nsis-header.bmp   (150x57, light bg, for installer pages)
  - icons/nsis-sidebar.bmp  (164x314, dark bg, for welcome/finish pages)

Requirements:
  - pip install Pillow
  - Inkscape installed (used for SVG → PNG rendering)
"""

import os
import subprocess
import tempfile
from PIL import Image, ImageDraw, ImageFont, ImageFilter

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPT_DIR)
SVG_ICON = os.path.join(SCRIPT_DIR, "kage-icon-basic.svg")   # app icon (PNGs, .ico)
SVG_NSIS = os.path.join(SCRIPT_DIR, "kage-icon.svg")          # NSIS installer images
OUT_DIR = SCRIPT_DIR

# Find Inkscape. Honour KAGE_INKSCAPE env var, then PATH, then common install locations.
import shutil as _shutil

_candidates = []
_env_inkscape = os.environ.get("KAGE_INKSCAPE")
if _env_inkscape:
    _candidates.append(_env_inkscape)
_path_inkscape = _shutil.which("inkscape")
if _path_inkscape:
    _candidates.append(_path_inkscape)
_candidates.extend([
    r"C:\Program Files\Inkscape\bin\inkscape.exe",
    r"C:\Program Files (x86)\Inkscape\bin\inkscape.exe",
    "/Applications/Inkscape.app/Contents/MacOS/inkscape",
    "/usr/bin/inkscape",
    "/usr/local/bin/inkscape",
])

INKSCAPE = None
for p in _candidates:
    try:
        subprocess.run([p, "--version"], capture_output=True, check=True)
        INKSCAPE = p
        break
    except (FileNotFoundError, subprocess.CalledProcessError, OSError):
        continue

if not INKSCAPE:
    raise RuntimeError(
        "Inkscape not found. Install it and ensure it is on PATH, "
        "or set the KAGE_INKSCAPE environment variable."
    )

# Theme colors (must match shared-kage-tokens.css)
OUTLINE_COLOR = (56, 178, 172)   # #38B2AC  --kage-mascot-outline
BG_DARK = (26, 32, 44)          # #1A202C  --kage-bg (dark)
BG_ELEVATED = (45, 55, 72)      # #2D3748  --kage-bg-elevated (dark)
ACCENT = (49, 151, 149)         # #319795  --kage-accent
TEXT_LIGHT = (226, 232, 240)    # #E2E8F0  --kage-text (dark)
TEXT_MUTED = (160, 174, 192)    # #A0AEC0  --kage-text-muted (dark)
TEXT_DARK = (45, 55, 72)        # #2D3748  --kage-text (light)


def svg_to_pil(svg_path, size, square=True):
    """Render SVG to a PIL Image using Inkscape.
    If square=True, centers on a square canvas with padding for outline.
    If square=False, renders at natural aspect ratio with width=size, padded for outline."""
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
        tmp_path = tmp.name
    try:
        if square:
            # Render at ~92% of canvas to leave just enough room for outline
            render_size = int(size * 0.92)
            subprocess.run([
                INKSCAPE, svg_path,
                "--export-type=png",
                f"--export-filename={tmp_path}",
                f"--export-width={render_size}",
                "--export-background-opacity=0",
            ], capture_output=True, check=True)
            rendered = Image.open(tmp_path).convert("RGBA")
            canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
            x = (size - rendered.width) // 2
            y = (size - rendered.height) // 2
            canvas.paste(rendered, (x, y), rendered)
            return canvas
        else:
            # Render at requested width, then add transparent padding for outline
            subprocess.run([
                INKSCAPE, svg_path,
                "--export-type=png",
                f"--export-filename={tmp_path}",
                f"--export-width={size}",
                "--export-background-opacity=0",
            ], capture_output=True, check=True)
            rendered = Image.open(tmp_path).convert("RGBA")
            pad = max(size // 20, 6)
            canvas = Image.new("RGBA", (rendered.width + pad * 2, rendered.height + pad * 2), (0, 0, 0, 0))
            canvas.paste(rendered, (pad, pad), rendered)
            return canvas
    finally:
        if os.path.exists(tmp_path):
            os.unlink(tmp_path)


def add_outline(img, color, radius=3):
    """Add a colored outline around non-transparent pixels."""
    alpha = img.split()[3]
    # Dilate the alpha mask to create the outline region
    dilated = alpha.copy()
    for _ in range(radius):
        dilated = dilated.filter(ImageFilter.MaxFilter(3))

    # Create outline layer
    outline = Image.new("RGBA", img.size, (*color, 255))
    outline.putalpha(dilated)

    # Composite: outline behind original
    result = Image.new("RGBA", img.size, (0, 0, 0, 0))
    result = Image.alpha_composite(result, outline)
    result = Image.alpha_composite(result, img)
    return result


def gen_app_icons():
    """Generate PNG icons and .ico for the app."""
    print("Rendering SVG at 512px...")
    hi_res = svg_to_pil(SVG_ICON, 512)
    hi_res_outlined = add_outline(hi_res, OUTLINE_COLOR, radius=6)

    for filename, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256)]:
        img = hi_res_outlined.resize((size, size), Image.LANCZOS)
        img.save(os.path.join(OUT_DIR, filename), "PNG")
        print(f"  {filename}")

    # ICO with multiple sizes
    ico_sizes = [16, 32, 48, 256]
    ico_frames = [hi_res_outlined.resize((s, s), Image.LANCZOS) for s in ico_sizes]
    ico_path = os.path.join(OUT_DIR, "kage.ico")
    # Pillow ICO: save the largest as base, append_images for the rest
    ico_frames[-1].save(ico_path, format="ICO", append_images=ico_frames[:-1])
    print(f"  kage.ico ({', '.join(str(s) for s in ico_sizes)})")


def gradient_fill(draw, width, height, top_color, bottom_color):
    """Draw a vertical gradient."""
    for y in range(height):
        t = y / max(height - 1, 1)
        r = int(top_color[0] + (bottom_color[0] - top_color[0]) * t)
        g = int(top_color[1] + (bottom_color[1] - top_color[1]) * t)
        b = int(top_color[2] + (bottom_color[2] - top_color[2]) * t)
        draw.line([(0, y), (width, y)], fill=(r, g, b))


def get_font(name, size):
    """Try to load a system font, fall back to default."""
    for font_name in [name, "segoeui.ttf", "arial.ttf"]:
        try:
            return ImageFont.truetype(font_name, size)
        except (OSError, IOError):
            continue
    return ImageFont.load_default()


def gen_nsis_sidebar():
    """164x314 sidebar image — dark gradient with outlined mascot."""
    w, h = 164, 314
    img = Image.new("RGB", (w, h))
    draw = ImageDraw.Draw(img)
    gradient_fill(draw, w, h, BG_DARK, BG_ELEVATED)

    # Accent stripe near bottom
    stripe_y = int(h * 0.82)
    for x in range(w):
        alpha = x / w
        r = int(ACCENT[0] * alpha)
        g = int(ACCENT[1] * alpha)
        b = int(ACCENT[2] * alpha)
        draw.line([(x, stripe_y), (x, stripe_y + 2)], fill=(r, g, b))

    # Mascot with outline — render at natural aspect ratio
    icon = svg_to_pil(SVG_NSIS, 400, square=False)
    icon = add_outline(icon, OUTLINE_COLOR, radius=4)
    icon_size = 80
    icon = icon.resize((icon_size, int(icon_size * icon.height / icon.width)), Image.LANCZOS)
    x = (w - icon.width) // 2
    y = int(h * 0.22) - icon.height // 2

    temp = img.convert("RGBA")
    temp.paste(icon, (x, y), icon)
    img = temp.convert("RGB")
    draw = ImageDraw.Draw(img)

    # Text
    font = get_font("segoeuib.ttf", 22)
    font_small = get_font("segoeui.ttf", 11)

    text = "Kage"
    bbox = draw.textbbox((0, 0), text, font=font)
    tw = bbox[2] - bbox[0]
    draw.text(((w - tw) // 2, int(h * 0.42)), text, fill=TEXT_LIGHT, font=font)

    sub = "AI Desktop Assistant"
    bbox2 = draw.textbbox((0, 0), sub, font=font_small)
    sw = bbox2[2] - bbox2[0]
    draw.text(((w - sw) // 2, int(h * 0.42) + 30), sub, fill=TEXT_MUTED, font=font_small)

    img.save(os.path.join(OUT_DIR, "nsis-sidebar.bmp"), "BMP")
    print("  nsis-sidebar.bmp")


def gen_nsis_header():
    """NSIS header image — rendered at 2x (300x114) for quality. NSIS downscales as needed."""
    w, h = 300, 114
    img = Image.new("RGB", (w, h), (255, 255, 255))

    # Mascot on the left — natural aspect ratio, sized to fit height
    icon = svg_to_pil(SVG_NSIS, 400, square=False)
    icon = add_outline(icon, OUTLINE_COLOR, radius=3)
    icon_h = 70
    icon = icon.resize((int(icon_h * icon.width / icon.height), icon_h), Image.LANCZOS)
    icon_x, icon_y = 12, (h - icon.height) // 2

    temp = img.convert("RGBA")
    temp.paste(icon, (icon_x, icon_y), icon)
    img = temp.convert("RGB")
    draw = ImageDraw.Draw(img)

    text_x = icon_x + icon.width + 12
    draw.text((text_x, 24), "Kage", fill=TEXT_DARK, font=get_font("segoeuib.ttf", 28))
    draw.text((text_x, 58), "AI Desktop Assistant", fill=(120, 120, 130), font=get_font("segoeui.ttf", 18))

    img.save(os.path.join(OUT_DIR, "nsis-header.bmp"), "BMP")
    print("  nsis-header.bmp")


if __name__ == "__main__":
    print("Generating app icons...")
    gen_app_icons()
    print("Generating NSIS installer images...")
    gen_nsis_sidebar()
    gen_nsis_header()
    print("Done!")

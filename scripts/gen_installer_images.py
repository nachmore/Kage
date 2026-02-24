"""Generate NSIS installer images for Kiro Assistant.

Sidebar: 164x314 BMP - shown on welcome/finish pages
Header:  150x57  BMP - shown on other installer pages
"""
from PIL import Image, ImageDraw, ImageFont
import os

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPT_DIR)
ICON_PATH = os.path.join(ROOT, "ui", "assets", "kiro-assistant-icon.png")
OUT_DIR = os.path.join(ROOT, "icons")

# Dark gradient colors matching the app theme
BG_TOP = (30, 26, 36)      # #1E1A24
BG_BOTTOM = (40, 36, 46)   # #28242E
ACCENT = (99, 102, 241)    # indigo accent

def gradient(draw, width, height, top_color, bottom_color):
    for y in range(height):
        r = int(top_color[0] + (bottom_color[0] - top_color[0]) * y / height)
        g = int(top_color[1] + (bottom_color[1] - top_color[1]) * y / height)
        b = int(top_color[2] + (bottom_color[2] - top_color[2]) * y / height)
        draw.line([(0, y), (width, y)], fill=(r, g, b))

def accent_stripe(draw, width, height):
    """Draw a subtle accent stripe near the bottom."""
    y_start = int(height * 0.82)
    stripe_h = 3
    for x in range(width):
        alpha = x / width
        r = int(ACCENT[0] * alpha)
        g = int(ACCENT[1] * alpha)
        b = int(ACCENT[2] * alpha)
        draw.line([(x, y_start), (x, y_start + stripe_h)], fill=(r, g, b))

def gen_sidebar():
    """164x314 sidebar image."""
    w, h = 164, 314
    img = Image.new("RGB", (w, h))
    draw = ImageDraw.Draw(img)
    gradient(draw, w, h, BG_TOP, BG_BOTTOM)

    # Accent stripe
    accent_stripe(draw, w, h)

    # Load and paste the icon centered, in the upper third
    try:
        icon = Image.open(ICON_PATH).convert("RGBA")
        icon_size = 80
        icon = icon.resize((icon_size, icon_size), Image.LANCZOS)
        # Center horizontally, place at ~25% from top
        x = (w - icon_size) // 2
        y = int(h * 0.22) - icon_size // 2
        # Composite onto the background
        temp = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        temp.paste(img.convert("RGBA"), (0, 0))
        temp.paste(icon, (x, y), icon)
        img = temp.convert("RGB")
        draw = ImageDraw.Draw(img)
    except Exception as e:
        print(f"Warning: Could not load icon: {e}")

    # Draw "Kiro" text below the icon
    try:
        font = ImageFont.truetype("segoeui.ttf", 22)
        font_small = ImageFont.truetype("segoeui.ttf", 11)
    except:
        font = ImageFont.load_default()
        font_small = font

    text = "Kiro"
    bbox = draw.textbbox((0, 0), text, font=font)
    tw = bbox[2] - bbox[0]
    tx = (w - tw) // 2
    ty = int(h * 0.42)
    draw.text((tx, ty), text, fill=(229, 231, 235), font=font)

    sub = "Assistant"
    bbox2 = draw.textbbox((0, 0), sub, font=font_small)
    sw = bbox2[2] - bbox2[0]
    sx = (w - sw) // 2
    draw.text((sx, ty + 30), sub, fill=(147, 143, 155), font=font_small)

    out = os.path.join(OUT_DIR, "nsis-sidebar.bmp")
    img.save(out, "BMP")
    print(f"Sidebar: {out}")

def gen_header():
    """150x57 header image."""
    w, h = 150, 57
    img = Image.new("RGB", (w, h), (240, 240, 240))  # Light background to match installer
    draw = ImageDraw.Draw(img)

    # Load and paste icon on the left
    icon_x = 8
    try:
        icon = Image.open(ICON_PATH).convert("RGBA")
        icon_size = 40
        icon = icon.resize((icon_size, icon_size), Image.LANCZOS)
        y = (h - icon_size) // 2
        temp = Image.new("RGBA", (w, h), (240, 240, 240, 255))
        temp.paste(icon, (icon_x, y), icon)
        img = temp.convert("RGB")
        draw = ImageDraw.Draw(img)
        icon_x += icon_size + 8
    except Exception as e:
        print(f"Warning: Could not load icon: {e}")
        icon_x = 12

    # "Kiro" on first line, "Assistant" on second — dark text on light bg
    try:
        font_title = ImageFont.truetype("segoeuib.ttf", 16)  # bold
        font_sub = ImageFont.truetype("segoeui.ttf", 13)
    except:
        font_title = ImageFont.load_default()
        font_sub = font_title

    draw.text((icon_x, 10), "Kiro", fill=(30, 26, 36), font=font_title)
    draw.text((icon_x, 29), "Assistant", fill=(100, 100, 110), font=font_sub)

    out = os.path.join(OUT_DIR, "nsis-header.bmp")
    img.save(out, "BMP")
    print(f"Header: {out}")

if __name__ == "__main__":
    gen_sidebar()
    gen_header()
    print("Done!")

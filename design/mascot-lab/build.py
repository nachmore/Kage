"""Build the self-contained mascot animation lab.

Inlines the real mascot SVGs from ui/assets into lab.template.html and writes
mascot-lab.html next to this script. Open the output straight from disk
(file://) — no dev server, no app build.

    python design/mascot-lab/build.py

Each SVG is reduced to its viewBox plus cleaned inner markup: Inkscape
metadata and ids are dropped, and the black / white fills are swapped for
the same body / eyes classes the app's mascot.js applies, so the lab themes
exactly like the app.
"""

import json
import re
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
ASSETS = ROOT / "ui" / "assets"

SETS = {
    "waving": [f"animations/waving/kage-waving-f{i}.svg" for i in range(1, 6)],
    "jumping": [f"animations/jumping/kage-jumping-f{i}.svg" for i in range(1, 9)],
    "poses": [
        "kage-happy.svg",
        "kage-winking.svg",
        "kage-interested.svg",
        "kage-looking-to-the-right.svg",
        "kage-sleeping.svg",
        "kage-magnifying-glass.svg",
        "kage-love.svg",
    ],
}

SKIP_TAGS = {"namedview", "metadata", "title", "desc"}
BLACK = {"#000000", "#000", "black"}
WHITE = {"#ffffff", "#fff", "white"}


def local(name):
    return name.rsplit("}", 1)[-1]


def esc(value):
    return (
        value.replace("&", "&amp;")
        .replace('"', "&quot;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
    )


def serialize(el):
    tag = local(el.tag)
    if tag in SKIP_TAGS:
        return ""
    attrs = {}
    classes = []
    for key, value in el.attrib.items():
        # Namespaced attributes are Inkscape/Sodipodi editor state; ids would
        # collide across the many inline copies the lab renders.
        if key.startswith("{") or key == "id":
            continue
        attrs[key] = value

    fill = attrs.pop("fill", None)
    style = attrs.pop("style", None)
    kept = []
    if style:
        for decl in style.split(";"):
            if ":" not in decl:
                continue
            prop, val = (part.strip() for part in decl.split(":", 1))
            if prop == "fill":
                fill = val
            else:
                kept.append(f"{prop}:{val}")
    if fill is not None:
        f = fill.lower()
        if f in BLACK:
            classes.append("m-body")
        elif f in WHITE:
            classes.append("m-eyes")
        else:
            kept.append(f"fill:{fill}")
    if kept:
        attrs["style"] = ";".join(kept)
    if classes:
        attrs["class"] = " ".join(classes)

    attr_str = "".join(f' {k}="{esc(v)}"' for k, v in attrs.items())
    children = "".join(serialize(child) for child in el)
    if children:
        return f"<{tag}{attr_str}>{children}</{tag}>"
    return f"<{tag}{attr_str}/>"


def load(rel):
    root = ET.parse(ASSETS / rel).getroot()
    view_box = [float(v) for v in re.split(r"[\s,]+", root.get("viewBox").strip())]
    inner = "".join(serialize(child) for child in root)
    name = Path(rel).stem.replace("kage-", "")
    return name, {"vb": view_box, "svg": inner}


def main():
    assets = {}
    for set_name, files in SETS.items():
        frames = {}
        for rel in files:
            name, data = load(rel)
            frames[name] = data
        assets[set_name] = frames

    template = (HERE / "lab.template.html").read_text(encoding="utf-8")
    payload = json.dumps(assets, separators=(",", ":"))
    out = template.replace("/*__ASSETS_JSON__*/null", payload)
    (HERE / "mascot-lab.html").write_text(out, encoding="utf-8")
    size_kb = len(out.encode("utf-8")) / 1024
    print(f"wrote {HERE / 'mascot-lab.html'} ({size_kb:.0f} KB)")


if __name__ == "__main__":
    main()

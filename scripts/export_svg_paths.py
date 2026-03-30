"""
Export each named path from kage-assets.svg into its own SVG file.
Each path has display:none in the original; we set it to display:inline.
Output files are named kage-<label-with-dashes>.svg in ui/assets/.
"""
import re
import os
import xml.etree.ElementTree as ET

INPUT = os.path.join(os.path.dirname(__file__), '..', 'ui', 'assets', 'kage-assets.svg')
OUTPUT_DIR = os.path.join(os.path.dirname(__file__), '..', 'ui', 'assets')

NS = {
    'svg': 'http://www.w3.org/2000/svg',
    'inkscape': 'http://www.inkscape.org/namespaces/inkscape',
    'sodipodi': 'http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd',
}

# Register namespaces so ET doesn't mangle them
for prefix, uri in NS.items():
    ET.register_namespace(prefix if prefix != 'svg' else '', uri)

tree = ET.parse(INPUT)
root = tree.getroot()

# Get the SVG attributes
width = root.get('width')
height = root.get('height')
viewBox = root.get('viewBox')

# Find the layer1 group and its transform
layer1 = root.find('.//{http://www.w3.org/2000/svg}g[@id="layer1"]')
layer_transform = layer1.get('transform', '') if layer1 is not None else ''

# Find all paths with inkscape:label
label_attr = f'{{{NS["inkscape"]}}}label'
paths = []
for path in root.iter(f'{{{NS["svg"]}}}path'):
    label = path.get(label_attr)
    if label:
        paths.append((label, path))

print(f"Found {len(paths)} named paths")

# Track used filenames to handle duplicates
used_names = {}

for label, path in paths:
    # Build filename: kage-<label with spaces replaced by dashes>.svg
    slug = label.lower().replace(' ', '-')
    if slug in used_names:
        used_names[slug] += 1
        filename = f'kage-{slug}-{used_names[slug]}.svg'
    else:
        used_names[slug] = 1
        filename = f'kage-{slug}.svg'
    filepath = os.path.join(OUTPUT_DIR, filename)

    # Get the path's style and make it visible
    style = path.get('style', '')
    new_style = style.replace('display:none', 'display:inline')
    
    # Get the path data
    d = path.get('d', '')
    path_id = path.get('id', '')

    # Build a standalone SVG
    svg_content = f'''<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<svg
   width="{width}"
   height="{height}"
   viewBox="{viewBox}"
   version="1.1"
   xmlns="http://www.w3.org/2000/svg">
  <g transform="{layer_transform}">
    <path
       style="{new_style}"
       d="{d}"
       id="{path_id}" />
  </g>
</svg>
'''
    with open(filepath, 'w', encoding='utf-8') as f:
        f.write(svg_content)
    print(f"  -> {filename}")

print("Done!")

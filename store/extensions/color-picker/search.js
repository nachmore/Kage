/**
 * Color Picker search provider — extracted from floating-color.js.
 * Detects hex, rgb, hsl, named colors and provides preview with format conversions.
 */

export default class ColorPickerSearchProvider {
    initialize(context) {
        this.config = context.config || {};
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    match(query) {
        const color = parseColor(query);
        if (!color) return [];
        const { r, g, b } = color;
        const hex = '#' + [r, g, b].map(c => c.toString(16).padStart(2, '0')).join('');
        return [{
            id: 'color:' + hex,
            type: 'color',
            label: hex.toUpperCase(),
            description: 'Color preview · Enter to copy',
            icon: '🎨',
            score: 95,
            data: color,
        }];
    }

    execute(result) {
        const { r, g, b } = result.data;
        const formats = formatAllColors(r, g, b);
        const fmt = this.config.copy_format || 'all';
        if (fmt === 'hex') return { type: 'copy', value: formats.hex };
        if (fmt === 'rgb') return { type: 'copy', value: formats.rgb };
        if (fmt === 'hsl') return { type: 'copy', value: formats.hsl };
        return { type: 'copy', value: `${formats.hex}\n${formats.rgb}\n${formats.hsl}` };
    }

    renderResult(result, element) {
        const { r, g, b } = result.data;
        const hex = rgbToHex(r, g, b);
        const formats = formatAllColors(r, g, b);
        element.innerHTML = `
            <div style="display:flex;align-items:center;gap:10px;padding:6px 0;">
                <div style="position:relative;width:32px;height:32px;flex-shrink:0;">
                    <div data-swatch style="background:${hex};border:2px solid rgba(255,255,255,0.2);border-radius:6px;width:100%;height:100%;"></div>
                    <input type="color" value="${hex}" data-color-picker style="position:absolute;top:0;left:0;width:100%;height:100%;opacity:0;cursor:pointer;border:none;padding:0;">
                </div>
                <div>
                    <div style="font-family:monospace;font-size:13px;color:var(--kiro-text-primary, #d4d4d4);" data-color-label>${formats.hex} · ${formats.rgb}</div>
                    <div style="font-family:monospace;font-size:12px;color:var(--kiro-text-muted, #9ca3af);" data-hsl-label>${formats.hsl}</div>
                </div>
            </div>
        `;
        const picker = element.querySelector('input[data-color-picker]');
        const swatch = element.querySelector('[data-swatch]');
        const label = element.querySelector('[data-color-label]');
        const hslLabel = element.querySelector('[data-hsl-label]');
        if (picker) {
            picker.addEventListener('click', (e) => e.stopPropagation());
            picker.addEventListener('input', (e) => {
                const newHex = e.target.value;
                if (swatch) swatch.style.background = newHex;
                const nr = parseInt(newHex.slice(1,3),16), ng = parseInt(newHex.slice(3,5),16), nb = parseInt(newHex.slice(5,7),16);
                result.data = { r: nr, g: ng, b: nb, source: 'picker' };
                result.label = newHex.toUpperCase();
                const f = formatAllColors(nr, ng, nb);
                if (label) label.textContent = `${f.hex} · ${f.rgb}`;
                if (hslLabel) hslLabel.textContent = f.hsl;
            });
        }
    }

    destroy() {}
}

// --- Color parsing/conversion (moved from floating-color.js) ---

function parseColor(input) {
    const trimmed = input.trim().toLowerCase();
    const hexMatch = trimmed.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/);
    if (hexMatch) {
        let hex = hexMatch[1];
        if (hex.length === 3) hex = hex[0]+hex[0]+hex[1]+hex[1]+hex[2]+hex[2];
        return { r: parseInt(hex.slice(0,2),16), g: parseInt(hex.slice(2,4),16), b: parseInt(hex.slice(4,6),16), source: 'hex' };
    }
    const rgbMatch = trimmed.match(/^rgb\(\s*(\d{1,3})\s*[,\s]\s*(\d{1,3})\s*[,\s]\s*(\d{1,3})\s*\)$/);
    if (rgbMatch) {
        const [, r, g, b] = rgbMatch.map(Number);
        if (r <= 255 && g <= 255 && b <= 255) return { r, g, b, source: 'rgb' };
    }
    const hslMatch = trimmed.match(/^hsl\(\s*(\d{1,3})\s*[,\s]\s*(\d{1,3})%?\s*[,\s]\s*(\d{1,3})%?\s*\)$/);
    if (hslMatch) {
        const [, h, s, l] = hslMatch.map(Number);
        if (h <= 360 && s <= 100 && l <= 100) {
            const { r, g, b } = hslToRgb(h, s, l);
            return { r, g, b, source: 'hsl' };
        }
    }
    const named = NAMED_COLORS[trimmed];
    if (named) return { ...named, source: 'name' };
    return null;
}

function rgbToHex(r, g, b) {
    return '#' + [r, g, b].map(c => c.toString(16).padStart(2, '0')).join('');
}

function rgbToHsl(r, g, b) {
    r /= 255; g /= 255; b /= 255;
    const max = Math.max(r, g, b), min = Math.min(r, g, b);
    let h, s, l = (max + min) / 2;
    if (max === min) { h = s = 0; }
    else {
        const d = max - min;
        s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
        switch (max) {
            case r: h = ((g - b) / d + (g < b ? 6 : 0)) / 6; break;
            case g: h = ((b - r) / d + 2) / 6; break;
            case b: h = ((r - g) / d + 4) / 6; break;
        }
    }
    return { h: Math.round(h * 360), s: Math.round(s * 100), l: Math.round(l * 100) };
}

function hslToRgb(h, s, l) {
    s /= 100; l /= 100;
    const c = (1 - Math.abs(2 * l - 1)) * s;
    const x = c * (1 - Math.abs((h / 60) % 2 - 1));
    const m = l - c / 2;
    let r, g, b;
    if (h < 60) { r = c; g = x; b = 0; }
    else if (h < 120) { r = x; g = c; b = 0; }
    else if (h < 180) { r = 0; g = c; b = x; }
    else if (h < 240) { r = 0; g = x; b = c; }
    else if (h < 300) { r = x; g = 0; b = c; }
    else { r = c; g = 0; b = x; }
    return { r: Math.round((r + m) * 255), g: Math.round((g + m) * 255), b: Math.round((b + m) * 255) };
}

function formatAllColors(r, g, b) {
    const hex = rgbToHex(r, g, b);
    const { h, s, l } = rgbToHsl(r, g, b);
    return {
        hex: hex.toUpperCase(),
        rgb: `rgb(${r}, ${g}, ${b})`,
        hsl: `hsl(${h}, ${s}%, ${l}%)`,
    };
}

const NAMED_COLORS = {
    red:{r:255,g:0,b:0},green:{r:0,g:128,b:0},blue:{r:0,g:0,b:255},
    white:{r:255,g:255,b:255},black:{r:0,g:0,b:0},yellow:{r:255,g:255,b:0},
    cyan:{r:0,g:255,b:255},magenta:{r:255,g:0,b:255},orange:{r:255,g:165,b:0},
    purple:{r:128,g:0,b:128},pink:{r:255,g:192,b:203},brown:{r:165,g:42,b:42},
    gray:{r:128,g:128,b:128},grey:{r:128,g:128,b:128},navy:{r:0,g:0,b:128},
    teal:{r:0,g:128,b:128},maroon:{r:128,g:0,b:0},olive:{r:128,g:128,b:0},
    lime:{r:0,g:255,b:0},aqua:{r:0,g:255,b:255},coral:{r:255,g:127,b:80},
    salmon:{r:250,g:128,b:114},gold:{r:255,g:215,b:0},silver:{r:192,g:192,b:192},
    indigo:{r:75,g:0,b:130},violet:{r:238,g:130,b:238},turquoise:{r:64,g:224,b:208},
    crimson:{r:220,g:20,b:60},khaki:{r:240,g:230,b:140},lavender:{r:230,g:230,b:250},
};

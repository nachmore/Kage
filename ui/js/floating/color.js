/**
 * Color detection, conversion, and picker for the floating window.
 * Detects hex, rgb, hsl color strings and shows a preview with format conversions.
 */

import { tHtml } from '../shared/i18n.js';

// --- Color detection ---

/**
 * Try to parse a color string. Returns { r, g, b, source } or null.
 */
export function parseColor(input) {
    const trimmed = input.trim().toLowerCase();

    // #rgb or #rrggbb
    const hexMatch = trimmed.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/);
    if (hexMatch) {
        let hex = hexMatch[1];
        if (hex.length === 3) hex = hex[0] + hex[0] + hex[1] + hex[1] + hex[2] + hex[2];
        return {
            r: parseInt(hex.slice(0, 2), 16),
            g: parseInt(hex.slice(2, 4), 16),
            b: parseInt(hex.slice(4, 6), 16),
            source: 'hex',
        };
    }

    // rgb(r, g, b) or rgb(r g b)
    const rgbMatch = trimmed.match(
        /^rgb\(\s*(\d{1,3})\s*[,\s]\s*(\d{1,3})\s*[,\s]\s*(\d{1,3})\s*\)$/
    );
    if (rgbMatch) {
        const [, r, g, b] = rgbMatch.map(Number);
        if (r <= 255 && g <= 255 && b <= 255) return { r, g, b, source: 'rgb' };
    }

    // hsl(h, s%, l%)
    const hslMatch = trimmed.match(
        /^hsl\(\s*(\d{1,3})\s*[,\s]\s*(\d{1,3})%?\s*[,\s]\s*(\d{1,3})%?\s*\)$/
    );
    if (hslMatch) {
        const [, h, s, l] = hslMatch.map(Number);
        if (h <= 360 && s <= 100 && l <= 100) {
            const { r, g, b } = hslToRgb(h, s, l);
            return { r, g, b, source: 'hsl' };
        }
    }

    // Named colors (common ones)
    const named = NAMED_COLORS[trimmed];
    if (named) return { ...named, source: 'name' };

    return null;
}

// --- Conversions ---

export function rgbToHex(r, g, b) {
    return '#' + [r, g, b].map((c) => c.toString(16).padStart(2, '0')).join('');
}

export function rgbToHsl(r, g, b) {
    r /= 255;
    g /= 255;
    b /= 255;
    const max = Math.max(r, g, b),
        min = Math.min(r, g, b);
    let h,
        s,
        l = (max + min) / 2;
    if (max === min) {
        h = s = 0;
    } else {
        const d = max - min;
        s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
        switch (max) {
            case r:
                h = ((g - b) / d + (g < b ? 6 : 0)) / 6;
                break;
            case g:
                h = ((b - r) / d + 2) / 6;
                break;
            case b:
                h = ((r - g) / d + 4) / 6;
                break;
        }
    }
    return { h: Math.round(h * 360), s: Math.round(s * 100), l: Math.round(l * 100) };
}

function hslToRgb(h, s, l) {
    s /= 100;
    l /= 100;
    const c = (1 - Math.abs(2 * l - 1)) * s;
    const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
    const m = l - c / 2;
    let r, g, b;
    if (h < 60) {
        r = c;
        g = x;
        b = 0;
    } else if (h < 120) {
        r = x;
        g = c;
        b = 0;
    } else if (h < 180) {
        r = 0;
        g = c;
        b = x;
    } else if (h < 240) {
        r = 0;
        g = x;
        b = c;
    } else if (h < 300) {
        r = x;
        g = 0;
        b = c;
    } else {
        r = c;
        g = 0;
        b = x;
    }
    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    };
}

export function formatAllColors(r, g, b) {
    const hex = rgbToHex(r, g, b);
    const { h, s, l } = rgbToHsl(r, g, b);
    return {
        hex: hex.toUpperCase(),
        rgb: `rgb(${r}, ${g}, ${b})`,
        hsl: `hsl(${h}, ${s}%, ${l}%)`,
    };
}

const NAMED_COLORS = {
    red: { r: 255, g: 0, b: 0 },
    green: { r: 0, g: 128, b: 0 },
    blue: { r: 0, g: 0, b: 255 },
    white: { r: 255, g: 255, b: 255 },
    black: { r: 0, g: 0, b: 0 },
    yellow: { r: 255, g: 255, b: 0 },
    cyan: { r: 0, g: 255, b: 255 },
    magenta: { r: 255, g: 0, b: 255 },
    orange: { r: 255, g: 165, b: 0 },
    purple: { r: 128, g: 0, b: 128 },
    pink: { r: 255, g: 192, b: 203 },
    brown: { r: 165, g: 42, b: 42 },
    gray: { r: 128, g: 128, b: 128 },
    grey: { r: 128, g: 128, b: 128 },
    navy: { r: 0, g: 0, b: 128 },
    teal: { r: 0, g: 128, b: 128 },
    maroon: { r: 128, g: 0, b: 0 },
    olive: { r: 128, g: 128, b: 0 },
    lime: { r: 0, g: 255, b: 0 },
    aqua: { r: 0, g: 255, b: 255 },
    coral: { r: 255, g: 127, b: 80 },
    salmon: { r: 250, g: 128, b: 114 },
    gold: { r: 255, g: 215, b: 0 },
    silver: { r: 192, g: 192, b: 192 },
    indigo: { r: 75, g: 0, b: 130 },
    violet: { r: 238, g: 130, b: 238 },
    turquoise: { r: 64, g: 224, b: 208 },
    crimson: { r: 220, g: 20, b: 60 },
    khaki: { r: 240, g: 230, b: 140 },
    lavender: { r: 230, g: 230, b: 250 },
};

// --- Rendering ---

/**
 * Render a color suggestion item into the suggestions container.
 * @returns {number} selectedIndex (0)
 */
export function renderColorSuggestion(
    color,
    container,
    currentMatches,
    onCopy,
    onPickerChange,
    resizeWindow
) {
    container.innerHTML = '';
    container.scrollTop = 0;
    currentMatches.length = 0;

    const { r, g, b } = color;
    const formats = formatAllColors(r, g, b);
    const hexValue = rgbToHex(r, g, b);

    currentMatches.push({ type: 'color', hex: formats.hex, rgb: formats.rgb, hsl: formats.hsl });

    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';
    item.style.cssText = 'flex-wrap: wrap; gap: 8px;';

    // Color swatch (clickable to open picker)
    const swatchWrapper = document.createElement('div');
    swatchWrapper.style.cssText = 'position:relative;cursor:pointer;';
    const swatch = document.createElement('div');
    swatch.className = 'app-icon';
    swatch.style.cssText = `background:${hexValue};border:2px solid rgba(255,255,255,0.2);border-radius:6px;width:32px;height:32px;`;
    const pickerInput = document.createElement('input');
    pickerInput.type = 'color';
    pickerInput.value = hexValue;
    pickerInput.style.cssText =
        'position:absolute;top:0;left:0;width:100%;height:100%;opacity:0;cursor:pointer;';
    pickerInput.addEventListener('input', (e) => {
        // Update swatch and formats in-place without rebuilding the DOM
        // (rebuilding would destroy the picker popup)
        const newHex = e.target.value;
        const nr = parseInt(newHex.slice(1, 3), 16),
            ng = parseInt(newHex.slice(3, 5), 16),
            nb = parseInt(newHex.slice(5, 7), 16);
        swatch.style.background = newHex;
        const newFormats = formatAllColors(nr, ng, nb);
        info.innerHTML = `
            <div class="app-name" style="font-family:monospace;font-size:13px;">${newFormats.hex} · ${newFormats.rgb}</div>
            <div class="app-description" style="font-family:monospace;">${newFormats.hsl}</div>
        `;
        // Update the stored match data for Enter-to-copy
        if (currentMatches.length > 0 && currentMatches[0].type === 'color') {
            currentMatches[0].hex = newFormats.hex;
            currentMatches[0].rgb = newFormats.rgb;
            currentMatches[0].hsl = newFormats.hsl;
        }
    });
    // When picker closes, update the input text to the final color
    pickerInput.addEventListener('change', (e) => {
        onPickerChange(e.target.value);
    });
    swatchWrapper.appendChild(swatch);
    swatchWrapper.appendChild(pickerInput);

    // Format info
    const info = document.createElement('div');
    info.className = 'app-info';
    info.style.minWidth = '0';
    info.innerHTML = `
        <div class="app-name" style="font-family:monospace;font-size:13px;">${formats.hex} · ${formats.rgb}</div>
        <div class="app-description" style="font-family:monospace;">${formats.hsl}</div>
    `;

    item.appendChild(swatchWrapper);
    item.appendChild(info);
    item.addEventListener('click', (e) => {
        if (e.target === pickerInput) return; // Don't copy when opening picker
        onCopy(formats);
    });

    container.appendChild(item);

    // Hint
    const hint = document.createElement('div');
    hint.className = 'suggestions-hint';
    hint.innerHTML = tHtml('floating.color.enter_to_copy_html');
    container.appendChild(hint);

    container.classList.add('visible');
    resizeWindow();
    return 0;
}

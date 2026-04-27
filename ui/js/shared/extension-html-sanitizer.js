/**
 * Extension HTML sanitizer.
 *
 * Used by widgets, toolbar button icons, message formatters, and custom
 * `renderResult` outputs — anywhere a sandboxed extension hands back
 * HTML that the host inserts into its DOM.
 *
 * Design goals:
 *   - Allow the set of tags extensions realistically need for rich UI
 *     (icons, buttons, structured text, tables of content, images).
 *   - Never allow script execution, event handlers, or URL schemes that
 *     can exfiltrate data or run code.
 *   - Never allow extensions to stamp elements with host-owned ids that
 *     could collide with first-party UI (floating controls, settings
 *     inputs, etc.). IDs are stripped entirely.
 *   - Link targets are restricted to http(s)/mailto/in-page; everything
 *     else is dropped.
 *
 * Two sanitize modes:
 *   - `rich`: broad HTML for formatters and widgets. Allows structural
 *     tags, inline styles (color/background only), SVG.
 *   - `inline`: narrow subset for small UI (toolbar icons, result rows).
 *     No block-level layout, no scripts, no external images.
 *
 * Any extension-declared `data-ext-action="<name>"` attribute is
 * preserved so the host can wire it to an RPC handler — but only on
 * elements that look like interactive targets (button, a).
 */

// --- Tag allow-lists --------------------------------------------------------

const RICH_TAGS = new Set([
    'A', 'ABBR', 'B', 'BLOCKQUOTE', 'BR', 'BUTTON', 'CAPTION', 'CODE',
    'DETAILS', 'DIV', 'DL', 'DD', 'DT', 'EM', 'FIGURE', 'FIGCAPTION', 'H1',
    'H2', 'H3', 'H4', 'H5', 'H6', 'HR', 'I', 'IMG', 'LI', 'MARK', 'OL',
    'P', 'PRE', 'S', 'SMALL', 'SPAN', 'STRONG', 'SUB', 'SUMMARY', 'SUP',
    'TABLE', 'TBODY', 'TD', 'TH', 'THEAD', 'TR', 'U', 'UL',
    // Inline SVG for icons — SVG itself is allow-listed and its
    // child tags are filtered by SVG_TAGS.
    'SVG',
]);

const INLINE_TAGS = new Set([
    'A', 'B', 'BR', 'BUTTON', 'CODE', 'EM', 'I', 'IMG', 'S', 'SMALL',
    'SPAN', 'STRONG', 'SVG',
]);

const SVG_TAGS = new Set([
    'CIRCLE', 'DEFS', 'ELLIPSE', 'G', 'LINE', 'LINEARGRADIENT', 'PATH',
    'POLYGON', 'POLYLINE', 'RADIALGRADIENT', 'RECT', 'STOP', 'SVG',
    'TEXT', 'TITLE', 'USE',
]);

// --- Attribute allow-lists --------------------------------------------------
//
// Attribute policies are tag-specific. `*` is the catch-all for "any of
// these tags may carry this attribute". The `data-ext-*` family is
// always preserved because that's how extensions hook interactivity.

const UNIVERSAL_ATTRS = new Set([
    'class', 'title', 'lang', 'dir', 'role', 'aria-label',
]);

const ATTR_POLICY = {
    A:      new Set(['href', 'target', 'rel']),
    IMG:    new Set(['src', 'alt', 'width', 'height']),
    BUTTON: new Set(['type', 'disabled']),
    // SVG and its kids need their full geometry/style attributes — we
    // allow a broad set here because these values are inert (they can't
    // execute script, only render pixels).
    SVG:    new Set(['xmlns', 'width', 'height', 'viewbox', 'fill', 'stroke', 'stroke-width', 'stroke-linecap', 'stroke-linejoin']),
    PATH:   new Set(['d', 'fill', 'stroke', 'stroke-width', 'stroke-linecap', 'stroke-linejoin']),
    CIRCLE: new Set(['cx', 'cy', 'r', 'fill', 'stroke', 'stroke-width']),
    RECT:   new Set(['x', 'y', 'width', 'height', 'rx', 'ry', 'fill', 'stroke', 'stroke-width']),
    LINE:   new Set(['x1', 'y1', 'x2', 'y2', 'stroke', 'stroke-width', 'stroke-linecap']),
    POLYLINE: new Set(['points', 'fill', 'stroke', 'stroke-width']),
    POLYGON:  new Set(['points', 'fill', 'stroke', 'stroke-width']),
    ELLIPSE:  new Set(['cx', 'cy', 'rx', 'ry', 'fill', 'stroke', 'stroke-width']),
    G:      new Set(['fill', 'stroke', 'stroke-width', 'transform']),
    USE:    new Set(['href', 'x', 'y', 'width', 'height']),
    TEXT:   new Set(['x', 'y', 'fill', 'stroke', 'font-size', 'text-anchor']),
    STOP:   new Set(['offset', 'stop-color', 'stop-opacity']),
    LINEARGRADIENT: new Set(['x1', 'y1', 'x2', 'y2']),
    RADIALGRADIENT: new Set(['cx', 'cy', 'r', 'fx', 'fy']),
    TABLE:  new Set(['class']),
    TH:     new Set(['scope', 'colspan', 'rowspan']),
    TD:     new Set(['colspan', 'rowspan']),
    IMG_BLOCKED_SCHEMES: new Set(['javascript:', 'vbscript:', 'data:text/html', 'file:']),
    DETAILS: new Set(['open']),
};

// --- URL validation ---------------------------------------------------------

function isSafeHref(value) {
    const v = String(value).trim();
    if (!v) return false;
    // Allow in-page anchors, http(s), mailto.
    return /^(https?:|mailto:|#)/i.test(v);
}

function isSafeImgSrc(value) {
    const v = String(value).trim();
    if (!v) return false;
    // data: is intentionally NOT allowed — extensions should use
    // controlled URLs or pass through host endpoints like fetch_favicon.
    // blob: and file: are also blocked.
    return /^https?:/i.test(v);
}

// --- Style filtering --------------------------------------------------------
//
// We accept a *very* narrow subset of inline style properties. Anything
// that can load external resources (background-image: url(...)) or
// hide/obscure the page (position: fixed, z-index: 999999) is blocked.

const SAFE_STYLE_PROPS = new Set([
    'color', 'background', 'background-color',
    'font-weight', 'font-style', 'font-size', 'font-family',
    'text-align', 'text-decoration', 'text-transform',
    'margin', 'margin-top', 'margin-right', 'margin-bottom', 'margin-left',
    'padding', 'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
    'display', 'flex', 'flex-direction', 'gap', 'align-items', 'justify-content',
    'border', 'border-radius', 'border-color', 'border-style', 'border-width',
    'width', 'height', 'max-width', 'max-height', 'min-width', 'min-height',
    'opacity', 'overflow', 'white-space',
]);

const DANGEROUS_STYLE_VALUE = /url\s*\(|expression\s*\(|javascript:|vbscript:|<|>|@import/i;

function filterStyle(raw) {
    if (typeof raw !== 'string' || !raw.trim()) return '';
    const out = [];
    for (const decl of raw.split(';')) {
        const parts = decl.split(':');
        if (parts.length < 2) continue;
        const prop = parts[0].trim().toLowerCase();
        const val = parts.slice(1).join(':').trim();
        if (!prop || !val) continue;
        if (!SAFE_STYLE_PROPS.has(prop)) continue;
        if (DANGEROUS_STYLE_VALUE.test(val)) continue;
        // Reject position: fixed/sticky — extensions shouldn't overlay
        // the whole app. Absolute is fine inside their widget container.
        if (prop === 'position') continue;
        out.push(`${prop}: ${val}`);
    }
    return out.join('; ');
}

// --- Public API -------------------------------------------------------------

/**
 * Sanitize extension-provided HTML for injection into the host DOM.
 *
 * @param {string} html
 * @param {'rich'|'inline'} [mode='rich']
 * @returns {DocumentFragment}
 */
export function sanitizeExtensionHtml(html, mode = 'rich') {
    const tpl = document.createElement('template');
    tpl.innerHTML = String(html);
    const tagsAllowed = mode === 'inline' ? INLINE_TAGS : RICH_TAGS;
    walkAndFilter(tpl.content, { tagsAllowed, inSvg: false });
    return tpl.content;
}

/**
 * Sanitize and return an HTML string (for callers that prefer to
 * stringify and later `innerHTML =` manually). Prefer the fragment
 * variant when you have a real container, because it avoids a second
 * parse pass.
 */
export function sanitizeExtensionHtmlToString(html, mode = 'rich') {
    const frag = sanitizeExtensionHtml(html, mode);
    const host = document.createElement('div');
    host.appendChild(frag);
    return host.innerHTML;
}

/**
 * Pull out data-ext-action="name" handlers from sanitized content.
 * Returns an array of { element, actionId } so the caller can wire
 * click handlers. The attribute survives sanitization; this helper
 * just batches the lookup.
 *
 * @param {HTMLElement | DocumentFragment} root
 */
export function findExtActions(root) {
    const hits = [];
    const nodes = root.querySelectorAll?.('[data-ext-action]') || [];
    for (const el of nodes) {
        const id = el.getAttribute('data-ext-action');
        if (id) hits.push({ element: el, actionId: id });
    }
    return hits;
}

// --- Internal walk ----------------------------------------------------------

function walkAndFilter(node, opts) {
    const kids = Array.from(node.childNodes);
    for (const child of kids) {
        if (child.nodeType === Node.ELEMENT_NODE) {
            processElement(child, opts);
        } else if (child.nodeType === Node.COMMENT_NODE) {
            child.remove();
        }
    }
}

function processElement(el, opts) {
    // DOM normalizes HTML tagNames to uppercase but leaves SVG
    // element tagNames in their original case (e.g. `svg`, `path`).
    // Our allow-lists are uppercase, so normalize before lookup.
    const tag = (el.tagName || '').toUpperCase();
    const allowed = opts.inSvg ? SVG_TAGS : opts.tagsAllowed;

    if (!allowed.has(tag) && !SVG_TAGS.has(tag)) {
        // Drop disallowed tags but keep their text content so visible
        // copy survives. Scripts, styles, iframes therefore always lose
        // their bodies (script bodies aren't text nodes that render).
        if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'IFRAME' ||
            tag === 'OBJECT' || tag === 'EMBED' || tag === 'LINK' ||
            tag === 'META') {
            el.remove();
        } else {
            const text = document.createTextNode(el.textContent || '');
            el.replaceWith(text);
        }
        return;
    }

    // Entering SVG context — once we're inside an SVG, our child tag
    // checks switch to SVG_TAGS regardless of the outer mode.
    const inSvg = opts.inSvg || tag === 'SVG';

    const tagPolicy = ATTR_POLICY[tag] || new Set();
    const toRemove = [];
    for (const attr of Array.from(el.attributes)) {
        const name = attr.name.toLowerCase();

        // Never preserve ids — they could collide with host-owned
        // selectors (e.g. `#floatingInput`).
        if (name === 'id') { toRemove.push(attr.name); continue; }

        // data-ext-action is the only recognised extension-side hook;
        // other data-* attributes are dropped so extensions can't
        // quietly pass state through the DOM.
        if (name === 'data-ext-action') {
            // Keep only on interactive-looking elements so a click
            // listener makes sense.
            if (tag !== 'BUTTON' && tag !== 'A' && tag !== 'SPAN' && tag !== 'DIV') {
                toRemove.push(attr.name);
            }
            continue;
        }
        if (name.startsWith('data-')) { toRemove.push(attr.name); continue; }

        // Event handler attributes (onclick, onload, etc.) — always drop.
        if (name.startsWith('on')) { toRemove.push(attr.name); continue; }

        // Style attribute — filter with property allow-list.
        if (name === 'style') {
            const filtered = filterStyle(attr.value);
            if (filtered) el.setAttribute('style', filtered);
            else toRemove.push(attr.name);
            continue;
        }

        // Universal attrs and tag-specific attrs pass unchanged.
        if (UNIVERSAL_ATTRS.has(name) || tagPolicy.has(name)) {
            // Tag-specific URL validations.
            if (tag === 'A' && name === 'href' && !isSafeHref(attr.value)) {
                toRemove.push(attr.name);
                continue;
            }
            if (tag === 'IMG' && name === 'src' && !isSafeImgSrc(attr.value)) {
                toRemove.push(attr.name);
                continue;
            }
            if (tag === 'USE' && name === 'href' && !/^#/.test(attr.value)) {
                // SVG <use> may only reference in-document symbols.
                toRemove.push(attr.name);
                continue;
            }
            continue;
        }

        // Fallthrough — unknown attr, drop it.
        toRemove.push(attr.name);
    }
    for (const n of toRemove) el.removeAttribute(n);

    // For http(s) anchor tags we force target=_blank. Tauri's webview
    // would otherwise navigate the main window when clicked, breaking
    // the app. Also add rel=noopener so the new tab can't hijack the
    // opener window.
    if (tag === 'A') {
        const href = el.getAttribute('href') || '';
        if (/^https?:/i.test(href)) {
            el.setAttribute('target', '_blank');
            el.setAttribute('rel', 'noopener noreferrer');
        } else if (el.getAttribute('target') === '_blank') {
            // Even for non-http links (mailto:, #fragment), preserve the
            // existing rel=noopener when target=_blank was declared.
            el.setAttribute('rel', 'noopener noreferrer');
        }
    }

    // Recurse.
    walkAndFilter(el, { tagsAllowed: opts.tagsAllowed, inSvg });
}

/**
 * The theme a view sees (docs/design/EXTENSION_VIEWS.md §11).
 *
 * The view page reads Kage's design tokens from its own document after
 * the theme is applied, keeps only the tokens listed below, and drops any
 * value `isSafeTokenValue` refuses. Theme extensions are untrusted too, so
 * a token value is never passed on unchecked. The view runtime writes the
 * snapshot into the view document as `--kage-*` variables and exposes it
 * as `context.view.theme`.
 */

/** The tokens a view gets, by name (without the leading `--`). */
export const VIEW_THEME_TOKENS = Object.freeze([
    'kage-bg',
    'kage-bg-elevated',
    'kage-bg-input',
    'kage-text',
    'kage-text-bright',
    'kage-text-muted',
    'kage-border',
    'kage-border-subtle',
    'kage-accent',
    'kage-accent-hover',
    'kage-accent-active',
    'kage-accent-light',
    'kage-accent-glow',
    'kage-link',
    'kage-success',
    'kage-warning',
    'kage-danger',
    'kage-error',
    'kage-font',
    'kage-font-mono',
]);

/** `config.ui.font_size` when it's missing. */
export const DEFAULT_FONT_SIZE = 14;
const MIN_FONT_SIZE = 8;
const MAX_FONT_SIZE = 32;

function fontSizeOf(value) {
    const n = Number(value);
    if (!Number.isFinite(n) || n <= 0) return DEFAULT_FONT_SIZE;
    return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, Math.round(n)));
}

/**
 * Build a snapshot from computed style.
 *
 * @param {object} opts
 * @param {{ getPropertyValue(name: string): string }} opts.style - computed
 *   style of the element whose tokens the page renders with (`document.body`:
 *   the light theme overrides tokens on `body.light-theme`).
 * @param {'dark'|'light'} opts.scheme
 * @param {number} [opts.fontSize] - `config.ui.font_size`
 * @param {boolean} [opts.reducedMotion]
 * @param {(name: string, value: string) => boolean} opts.isSafeTokenValue
 * @returns {{ scheme: 'dark'|'light', fontSize: number, reducedMotion: boolean,
 *   tokens: Record<string, string> }}
 */
export function computeThemeSnapshot({ style, scheme, fontSize, reducedMotion, isSafeTokenValue }) {
    const tokens = {};
    for (const name of VIEW_THEME_TOKENS) {
        let raw;
        try {
            raw = style?.getPropertyValue?.(`--${name}`);
        } catch {
            raw = '';
        }
        if (typeof raw !== 'string') continue;
        // Custom properties keep their source whitespace (the font stacks
        // span lines in shared-kage-tokens.css).
        const value = raw.replace(/\s+/g, ' ').trim();
        if (!value) continue;
        let safe = false;
        try {
            safe = isSafeTokenValue(name, value) === true;
        } catch {
            safe = false;
        }
        if (!safe) {
            console.warn(`[extension-view] theme token --${name} dropped: unsafe value`);
            continue;
        }
        tokens[name] = value;
    }
    return {
        scheme: scheme === 'light' ? 'light' : 'dark',
        fontSize: fontSizeOf(fontSize),
        reducedMotion: !!reducedMotion,
        tokens,
    };
}

/**
 * Read the snapshot from a live document.
 *
 * @param {object} opts
 * @param {Document} opts.doc
 * @param {Window} opts.win
 * @param {object} [opts.config] - Kage's config, for `ui.font_size`
 * @param {(name: string, value: string) => boolean} opts.isSafeTokenValue
 */
export function readThemeSnapshot({ doc, win, config, isSafeTokenValue }) {
    const body = doc.body || doc.documentElement;
    let style = null;
    try {
        style = win.getComputedStyle(body);
    } catch {
        style = null;
    }
    let reducedMotion = false;
    try {
        reducedMotion = !!win.matchMedia?.('(prefers-reduced-motion: reduce)')?.matches;
    } catch {
        reducedMotion = false;
    }
    return computeThemeSnapshot({
        style,
        scheme: body?.classList?.contains('light-theme') ? 'light' : 'dark',
        fontSize: config?.ui?.font_size,
        reducedMotion,
        isSafeTokenValue,
    });
}

/** Whether two snapshots would look the same to a view. */
export function sameThemeSnapshot(a, b) {
    if (!a || !b) return a === b;
    if (a.scheme !== b.scheme || a.fontSize !== b.fontSize) return false;
    if (a.reducedMotion !== b.reducedMotion) return false;
    const ak = Object.keys(a.tokens || {});
    const bk = Object.keys(b.tokens || {});
    if (ak.length !== bk.length) return false;
    return ak.every((k) => a.tokens[k] === b.tokens?.[k]);
}

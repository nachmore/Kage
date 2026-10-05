/**
 * Lazy loader for Prism syntax-highlighting language packs.
 *
 * Eagerly loading every language pack (15 of them, ~75 KB) at every
 * window startup was wasteful — most responses contain at most one
 * fenced language, and many sessions touch zero. Instead, ship only
 * `prism.js` (the runtime that defines `Prism.languages` and
 * `Prism.highlight`) and load each language's component on first use.
 *
 * Usage:
 *   import { loadPrismLanguage } from './prism-loader.js';
 *   await loadPrismLanguage('typescript');
 *   if (Prism.languages.typescript) Prism.highlight(...);
 *
 * Concurrent calls for the same language share one fetch. Already-loaded
 * languages resolve synchronously. Names with no shipped pack reject
 * immediately without touching the DOM. Load failures of a shipped pack
 * reject but do *not* poison the cache — a retry on the next code block
 * gets a fresh attempt.
 */

/**
 * Component dependency graph from upstream Prism. The pack files use
 * `Prism.languages.extend("base", ...)` so the base must be loaded
 * first or the call throws. Maintained here rather than parsing the
 * pack files because the graph is small and stable.
 */
const PRISM_DEPENDENCIES = {
    javascript: ['clike'],
    typescript: ['clike', 'javascript'],
    csharp: ['clike'],
    java: ['clike'],
    go: ['clike'],
    rust: ['clike'],
    css: ['markup'],
    markdown: ['markup'],
    // bash, json, sql, yaml, python, clike, markup are self-contained.
};

/**
 * Packs that actually ship under vendor/lib/prism-components/. Keep in
 * sync with PRISM_LANGUAGES in ui-vendor/setup.js. Anything else would
 * 404, so we never inject a <script> for it — otherwise every streaming
 * re-render of e.g. a ```sh block appended another dead tag to <head>.
 */
const SHIPPED_PACKS = new Set([
    'bash',
    'clike',
    'csharp',
    'css',
    'go',
    'java',
    'javascript',
    'json',
    'markdown',
    'markup',
    'python',
    'rust',
    'sql',
    'typescript',
    'yaml',
]);

/**
 * Common fence tags → canonical pack name. Prism only registers these
 * aliases once the canonical pack has loaded, so `Prism.languages.py` is
 * undefined until something loads `python`.
 */
const PRISM_ALIASES = {
    py: 'python',
    python3: 'python',
    ts: 'typescript',
    sh: 'bash',
    shell: 'bash',
    zsh: 'bash',
    console: 'bash',
    yml: 'yaml',
    cs: 'csharp',
    rs: 'rust',
    golang: 'go',
    js: 'javascript',
    md: 'markdown',
    jsonc: 'json',
    html: 'markup',
    htm: 'markup',
    xml: 'markup',
    svg: 'markup',
};

/**
 * Map a fence tag to the shipped pack that provides it, or null if no
 * shipped pack does (caller should leave the block unhighlighted).
 */
export function resolvePrismLanguage(language) {
    if (!language) return null;
    const lower = String(language).toLowerCase();
    const canonical = PRISM_ALIASES[lower] || lower;
    return SHIPPED_PACKS.has(canonical) ? canonical : null;
}

/** Map of language name → Promise resolving when load is complete. */
const _inflight = new Map();

/**
 * Load a language pack. Returns a promise that resolves once the pack
 * (and any deps) are present on `Prism.languages`. Synchronous resolve
 * if already loaded.
 */
export function loadPrismLanguage(language) {
    if (typeof window === 'undefined' || !window.Prism) {
        return Promise.reject(new Error('Prism core not loaded'));
    }
    if (window.Prism.languages[language]) {
        return Promise.resolve();
    }
    if (!SHIPPED_PACKS.has(language)) {
        // Permanent miss — no pack to fetch, so no retry either.
        return Promise.reject(new Error(`No Prism pack shipped for '${language}'`));
    }
    const existing = _inflight.get(language);
    if (existing) return existing;

    const promise = (async () => {
        // Load deps first, sequentially — each pack does
        // Prism.languages.extend(<dep>, …) at script-eval time, so the
        // dep must be present before the dependent pack runs.
        const deps = PRISM_DEPENDENCIES[language] || [];
        for (const dep of deps) {
            await loadPrismLanguage(dep);
        }
        await _injectScript(`vendor/lib/prism-components/prism-${language}.min.js`);
        if (!window.Prism.languages[language]) {
            throw new Error(`Prism pack '${language}' loaded but did not register`);
        }
    })();

    _inflight.set(language, promise);
    promise.catch(() => {
        // On failure, drop the cached promise so a later code block
        // (e.g. after the network comes back) can retry.
        _inflight.delete(language);
    });
    return promise;
}

function _injectScript(src) {
    return new Promise((resolve, reject) => {
        const s = document.createElement('script');
        s.src = src;
        s.async = false; // preserve eval order vs. other in-flight injects
        s.onload = () => resolve();
        s.onerror = () => {
            // Drop the dead tag so retries don't accumulate them in <head>.
            s.remove();
            reject(new Error(`Failed to load ${src}`));
        };
        document.head.appendChild(s);
    });
}

/** Test-only: drop the in-flight cache so each test starts fresh. */
export function _resetForTests() {
    _inflight.clear();
}

/**
 * Language detection utility — combines Unicode script detection with tinyld
 * for accurate language identification across all text lengths.
 *
 * Strategy:
 * 1. Unicode script detection (instant, works for single words, definitive for unique scripts)
 * 2. tinyld n-gram detection (statistical, better for Latin/Cyrillic with longer text)
 * 3. Fallback to empty string if nothing matches
 *
 * Usage:
 *   import { detectLanguage, detectLanguageAll, detectScript } from '../shared/language-detect.js';
 *
 *   const lang = await detectLanguage('مرحبا');        // 'ar' (script detection)
 *   const lang2 = await detectLanguage('bonjour');     // 'fr' (tinyld)
 *   const script = detectScript('こんにちは');           // 'ja' (synchronous)
 */

// --- Unicode script → language mapping ---
// For scripts used by exactly one major language, the mapping is definitive.
// For shared scripts (Latin, Cyrillic), we return null and let tinyld handle it.
const SCRIPT_RANGES = [
    // East Asian
    { range: [0x3040, 0x309F], lang: 'ja' },  // Hiragana
    { range: [0x30A0, 0x30FF], lang: 'ja' },  // Katakana
    { range: [0x4E00, 0x9FFF], lang: 'zh' },  // CJK Unified Ideographs (Chinese default, also used in Japanese)
    { range: [0xAC00, 0xD7AF], lang: 'ko' },  // Hangul Syllables
    { range: [0x1100, 0x11FF], lang: 'ko' },  // Hangul Jamo

    // South/Southeast Asian
    { range: [0x0E00, 0x0E7F], lang: 'th' },  // Thai
    { range: [0x0900, 0x097F], lang: 'hi' },  // Devanagari (Hindi default)
    { range: [0x0980, 0x09FF], lang: 'bn' },  // Bengali
    { range: [0x0A80, 0x0AFF], lang: 'gu' },  // Gujarati
    { range: [0x0B00, 0x0B7F], lang: 'or' },  // Odia
    { range: [0x0B80, 0x0BFF], lang: 'ta' },  // Tamil
    { range: [0x0C00, 0x0C7F], lang: 'te' },  // Telugu
    { range: [0x0C80, 0x0CFF], lang: 'kn' },  // Kannada
    { range: [0x0D00, 0x0D7F], lang: 'ml' },  // Malayalam
    { range: [0x0A00, 0x0A7F], lang: 'pa' },  // Gurmukhi (Punjabi)
    { range: [0x1000, 0x109F], lang: 'my' },  // Myanmar (Burmese)
    { range: [0x0E80, 0x0EFF], lang: 'lo' },  // Lao
    { range: [0x1780, 0x17FF], lang: 'km' },  // Khmer

    // Middle Eastern
    { range: [0x0590, 0x05FF], lang: 'he' },  // Hebrew
    { range: [0x0600, 0x06FF], lang: 'ar' },  // Arabic
    { range: [0xFB50, 0xFDFF], lang: 'ar' },  // Arabic Presentation Forms-A
    { range: [0xFE70, 0xFEFF], lang: 'ar' },  // Arabic Presentation Forms-B
    { range: [0x0530, 0x058F], lang: 'hy' },  // Armenian
    { range: [0x10A0, 0x10FF], lang: 'ka' },  // Georgian

    // Other
    { range: [0x0F00, 0x0FFF], lang: 'bo' },  // Tibetan
    { range: [0x1200, 0x137F], lang: 'am' },  // Ethiopic (Amharic default)
];

/**
 * Detect language from Unicode script (synchronous, instant).
 * Returns an ISO 639-1 code for unique scripts, or null for Latin/Cyrillic/ambiguous.
 * @param {string} text
 * @returns {string|null}
 */
export function detectScript(text) {
    if (!text) return null;

    // Count script hits
    const hits = new Map();
    for (const char of text) {
        const cp = char.codePointAt(0);
        for (const { range, lang } of SCRIPT_RANGES) {
            if (cp >= range[0] && cp <= range[1]) {
                hits.set(lang, (hits.get(lang) || 0) + 1);
                break;
            }
        }
    }

    if (hits.size === 0) return null;

    // Return the script with the most character hits
    let best = null, bestCount = 0;
    for (const [lang, count] of hits) {
        if (count > bestCount) { best = lang; bestCount = count; }
    }
    return best;
}

// --- tinyld lazy loader ---
let _tinyld = null;
let _loading = null;

async function _ensureLoaded() {
    if (_tinyld) return _tinyld;
    if (_loading) return _loading;
    _loading = import('../../vendor/lib/tinyld.js').then(mod => {
        _tinyld = mod;
        _loading = null;
        return mod;
    });
    return _loading;
}

/**
 * Detect the most likely language of the given text.
 * Uses script detection first (definitive for unique scripts),
 * then falls back to tinyld for Latin/Cyrillic text.
 * @param {string} text - Text to analyze
 * @param {object} [options] - Options: { only: ['en', 'fr', ...] } to restrict candidates
 * @returns {Promise<string>} ISO 639-1 language code or empty string
 */
export async function detectLanguage(text, options) {
    if (!text || text.trim().length < 2) return '';

    // 1. Try Unicode script detection (instant, definitive for unique scripts)
    const scriptLang = detectScript(text);
    if (scriptLang) return scriptLang;

    // 2. Fall back to tinyld for Latin/Cyrillic/ambiguous scripts
    try {
        const mod = await _ensureLoaded();
        return mod.detect(text, options);
    } catch {
        return '';
    }
}

/**
 * Detect all candidate languages with confidence scores.
 * @param {string} text - Text to analyze
 * @param {object} [options] - Options: { only: ['en', 'fr', ...] } to restrict candidates
 * @returns {Promise<Array<{lang: string, accuracy: number}>>} Sorted by accuracy descending
 */
export async function detectLanguageAll(text, options) {
    if (!text || text.trim().length < 3) return [];

    // For unique scripts, return a single high-confidence result
    const scriptLang = detectScript(text);
    if (scriptLang) return [{ lang: scriptLang, accuracy: 1.0 }];

    try {
        const mod = await _ensureLoaded();
        return mod.detectAll(text, options);
    } catch {
        return [];
    }
}

/**
 * Get the human-readable name for a language code.
 * @param {string} code - ISO 639-1 language code
 * @returns {Promise<string>} Language name or empty string
 */
export async function getLanguageName(code) {
    const mod = await _ensureLoaded();
    return mod.langName?.(code) || '';
}

/**
 * Get the list of supported language codes (tinyld).
 * @returns {Promise<string[]>}
 */
export async function getSupportedLanguages() {
    const mod = await _ensureLoaded();
    return mod.supportedLanguages || [];
}

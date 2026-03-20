/**
 * Language detection utility — lazy-loads tinyld for on-demand language detection.
 * Available app-wide for extensions, chat, floating window, etc.
 *
 * Usage:
 *   import { detectLanguage, detectLanguageAll } from '../shared/language-detect.js';
 *
 *   const lang = await detectLanguage('これは日本語です');  // 'ja'
 *   const all = await detectLanguageAll('ceci est un texte en français');
 *   // [{ lang: 'fr', accuracy: 0.52 }, { lang: 'ro', accuracy: 0.38 }, ...]
 */

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
 * @param {string} text - Text to analyze (longer text = more accurate)
 * @param {object} [options] - Options: { only: ['en', 'fr', ...] } to restrict candidates
 * @returns {Promise<string>} ISO 639-1 language code (e.g. 'en', 'fr', 'ja') or empty string
 */
export async function detectLanguage(text, options) {
    if (!text || text.trim().length < 3) return '';
    const mod = await _ensureLoaded();
    return mod.detect(text, options);
}

/**
 * Detect all candidate languages with confidence scores.
 * @param {string} text - Text to analyze
 * @param {object} [options] - Options: { only: ['en', 'fr', ...] } to restrict candidates
 * @returns {Promise<Array<{lang: string, accuracy: number}>>} Sorted by accuracy descending
 */
export async function detectLanguageAll(text, options) {
    if (!text || text.trim().length < 3) return [];
    const mod = await _ensureLoaded();
    return mod.detectAll(text, options);
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
 * Get the list of supported language codes.
 * @returns {Promise<string[]>} Array of ISO 639-3 language codes
 */
export async function getSupportedLanguages() {
    const mod = await _ensureLoaded();
    return mod.supportedLanguages || [];
}

/**
 * Dictionary search provider — looks up word definitions via FreeDictionaryAPI.com.
 * Uses match() for instant "loading" feedback and matchAsync() for the actual API call.
 */

const API_BASE = 'https://freedictionaryapi.com/api/v1/entries';
const SUGGEST_API = 'https://api.datamuse.com/sug';
const MIN_WORD_LENGTH = 2;

// Lazy-load language detection
let _detectLanguage = null;
async function _getDetector() {
    if (_detectLanguage) return _detectLanguage;
    try {
        // Use absolute URL — extensions may be loaded from blob URLs where relative imports fail
        const base = new URL('/', location.href).href;
        const mod = await import(base + 'js/shared/language-detect.js');
        _detectLanguage = mod.detectLanguage;
        return _detectLanguage;
    } catch (e) {
        console.warn('[Dictionary] Language detection not available:', e);
        return null;
    }
}

export default class DictionarySearchProvider {
    initialize(context) {
        this.config = context.config || {};
        this._cache = new Map();
        this._suggestCache = new Map();
    }

    onConfigUpdate(config) {
        this.config = config || {};
        this._cache.clear();
        this._suggestCache.clear();
    }

    match(query) {
        const word = this._extractWord(query);
        if (!word || !this._isLookupCandidate(word)) return [];

        const lang = this.config.language || 'auto';

        if (lang === 'auto') {
            // Check if any language has this word cached
            for (const [key, val] of this._cache) {
                if (key.endsWith(':' + word) && val !== 'not_found') {
                    return this._formatResults(val, word);
                }
            }
            return [];
        }

        const cacheKey = `${lang}:${word}`;
        const cached = this._cache.get(cacheKey);
        if (cached === 'not_found') {
            const suggestCached = this._suggestCache.get(word);
            if (suggestCached) return suggestCached;
            return [this._notFoundResult(word)];
        }
        if (cached) return this._formatResults(cached, word);

        return [];
    }

    async matchAsync(query) {
        const word = this._extractWord(query);
        if (!word) {
            console.log(`[Dictionary] No word extracted from query: "${query}"`);
            return [];
        }
        if (!this._isLookupCandidate(word)) {
            console.log(`[Dictionary] Not a lookup candidate: "${word}"`);
            return [];
        }

        // Resolve language: auto-detect or use configured
        let lang = this.config.language || 'auto';
        if (lang === 'auto') {
            const detect = await _getDetector();
            if (detect) {
                const detected = await detect(word);
                lang = detected || 'en'; // Fall back to English
                console.log(`[Dictionary] Auto-detected language for "${word}": ${detected ? `"${detected}"` : '(none)'} → using "${lang}"`);
            } else {
                lang = 'en';
                console.log(`[Dictionary] Language detection unavailable, falling back to "en"`);
            }
        } else {
            console.log(`[Dictionary] Using configured language "${lang}" for "${word}"`);
        }

        const cacheKey = `${lang}:${word}`;

        // Already cached (hit or miss) — match() handled it
        if (this._cache.has(cacheKey)) return [];

        // Fetch definition
        try {
            const url = `${API_BASE}/${encodeURIComponent(lang)}/${encodeURIComponent(word)}`;
            console.log(`[Dictionary] Fetching: ${url}`);
            const data = await this._fetchDefinition(word, lang);
            console.log(`[Dictionary] Result for "${word}" (${lang}): ${data?.entries?.length ?? 0} entries`);
            if (data && data.entries && data.entries.length > 0) {
                this._cache.set(cacheKey, data);
                return this._formatResults(data, word);
            }

            // Not found in detected language — try English as fallback
            if (lang !== 'en') {
                const enKey = `en:${word}`;
                if (!this._cache.has(enKey)) {
                    const enData = await this._fetchDefinition(word, 'en');
                    if (enData && enData.entries && enData.entries.length > 0) {
                        this._cache.set(enKey, enData);
                        return this._formatResults(enData, word);
                    }
                }
            }

            // Not found — try spelling suggestions (English only)
            this._cache.set(cacheKey, 'not_found');
            const suggestions = await this._fetchSpellingSuggestions(word);
            if (suggestions.length > 0) return suggestions;
            return [this._notFoundResult(word)];
        } catch (e) {
            console.warn('[Dictionary] Lookup failed:', e);
            return [{
                id: 'dict-error',
                type: 'dictionary',
                label: `📖 Could not look up "${word}"`,
                description: 'Network error — check your connection',
                icon: '📖',
                score: 85,
                data: { type: 'error', word },
            }];
        }
    }

    execute(result) {
        if (result.data?.type === 'loading' || result.data?.type === 'error' || result.data?.type === 'not_found') {
            return null; // No action for loading/error/not-found states
        }
        if (result.data?.type === 'suggestion') {
            // Replace input with trigger + suggested word so it triggers a new lookup
            const trigger = (this.config.trigger ?? 'dict').trim();
            const newInput = trigger ? `${trigger} ${result.data.word}` : result.data.word;
            return { type: 'replace_input', value: newInput };
        }
        return { type: 'copy', value: result.data?.copyText || result.label };
    }

    renderResult(result, element) {
        if (result.data?.type === 'definition') {
            element.innerHTML = this._renderDefinitionHtml(result.data);
            return true;
        }
        if (result.data?.type === 'suggestion') {
            element.innerHTML = this._renderSuggestionHtml(result.data);
            return true;
        }
        if (result.data?.type === 'loading') {
            element.innerHTML = `<div class="dict-result"><span class="dict-icon">📖</span><span class="dict-word">Looking up "${_escHtml(result.data.word)}"...</span></div>`;
            return true;
        }
        if (result.data?.type === 'not_found') {
            element.innerHTML = `<div class="dict-result"><span class="dict-icon">📖</span><span class="dict-word">No definition found for "${_escHtml(result.data.word)}"</span></div>`;
            return true;
        }
        return false;
    }

    destroy() {
        this._cache.clear();
        this._suggestCache.clear();
    }

    // --- Private helpers ---

    _extractWord(query) {
        const trimmed = query.trim();
        const trigger = (this.config.trigger ?? 'dict').trim().toLowerCase();

        if (!trigger) {
            return trimmed.toLowerCase();
        }

        const lower = trimmed.toLowerCase();
        if (!lower.startsWith(trigger + ' ')) return null;

        const word = trimmed.slice(trigger.length).trim().toLowerCase();
        return word || null;
    }

    _isLookupCandidate(query) {
        if (query.length < MIN_WORD_LENGTH) return false;
        if (query.startsWith('>') || query.startsWith('/')) return false;
        // Skip if it looks like math, a URL, or a file path
        if (/[+\-*\/=<>{}()\[\]\\|@#$%^&]/.test(query)) return false;
        if (/^https?:\/\//.test(query)) return false;
        if (/\.\w{1,6}$/.test(query)) return false;
        // Skip if it's mostly numbers
        if (/^\d+$/.test(query)) return false;
        // Allow up to 2 words (any script)
        const words = query.trim().split(/\s+/);
        if (words.length > 2) return false;
        return true;
    }

    _notFoundResult(word) {
        return {
            id: 'dict-not-found',
            type: 'dictionary',
            label: `No definition found for "${word}"`,
            description: 'Try a different spelling',
            icon: '📖',
            score: 85,
            data: { type: 'not_found', word },
        };
    }

    async _fetchDefinition(word, lang) {
        const url = `${API_BASE}/${encodeURIComponent(lang)}/${encodeURIComponent(word)}`;
        const resp = await fetch(url);
        if (!resp.ok) return null;
        return await resp.json();
    }

    async _fetchSpellingSuggestions(word) {
        if (this._suggestCache.has(word)) {
            return this._suggestCache.get(word);
        }
        try {
            const url = `${SUGGEST_API}?s=${encodeURIComponent(word)}&max=5`;
            const resp = await fetch(url);
            if (!resp.ok) return [];
            const suggestions = await resp.json();

            const results = suggestions
                .filter(s => s.word.toLowerCase() !== word)
                .slice(0, 3)
                .map((s, i) => ({
                    id: `dict-suggest:${s.word}`,
                    type: 'dictionary',
                    label: `Did you mean: ${s.word}?`,
                    description: 'Press Enter to copy corrected word',
                    icon: '📖',
                    score: 82 - i,
                    data: { type: 'suggestion', word: s.word },
                }));

            this._suggestCache.set(word, results);
            return results;
        } catch {
            return [];
        }
    }

    _formatResults(data, query) {
        const results = [];
        const seen = new Set();

        for (const entry of data.entries) {
            const pos = entry.partOfSpeech || '';
            const langName = entry.language?.name || '';
            const key = `${pos}:${entry.senses?.[0]?.definition || ''}`;
            if (seen.has(key)) continue;
            seen.add(key);

            const firstSense = entry.senses?.[0];
            if (!firstSense?.definition) continue;

            const pronunciation = this.config.show_pronunciation !== false
                ? (entry.pronunciations?.[0]?.text || '')
                : '';

            const synonyms = this.config.show_synonyms !== false
                ? (firstSense.synonyms || entry.synonyms || []).slice(0, 4)
                : [];

            const examples = this.config.show_examples !== false
                ? (firstSense.examples || []).slice(0, 1)
                : [];

            const allDefinitions = entry.senses
                ?.filter(s => s.definition)
                .slice(0, 3)
                .map(s => s.definition) || [];

            const copyText = `${data.word} (${pos}): ${allDefinitions.join('; ')}`;

            results.push({
                id: `dict:${data.word}:${pos}`,
                type: 'dictionary',
                label: `${data.word} — ${firstSense.definition}`,
                description: pos + (pronunciation ? ` · ${pronunciation}` : ''),
                icon: '📖',
                score: 85,
                data: {
                    type: 'definition',
                    word: data.word,
                    partOfSpeech: pos,
                    langName,
                    pronunciation,
                    definitions: allDefinitions,
                    synonyms,
                    examples,
                    copyText,
                    sourceUrl: data.source?.url || '',
                },
            });

            if (results.length >= 3) break;
        }

        return results;
    }

    _renderDefinitionHtml(data) {
        const posTag = data.partOfSpeech
            ? `<span class="dict-pos">${_escHtml(data.partOfSpeech)}</span>` : '';
        const pronTag = data.pronunciation
            ? `<span class="dict-pron">${_escHtml(data.pronunciation)}</span>` : '';

        let defsHtml = '';
        for (let i = 0; i < data.definitions.length; i++) {
            defsHtml += `<div class="dict-def">${i + 1}. ${_escHtml(data.definitions[i])}</div>`;
        }

        let synHtml = '';
        if (data.synonyms.length > 0) {
            synHtml = `<div class="dict-syn">Syn: ${data.synonyms.map(s => _escHtml(s)).join(', ')}</div>`;
        }

        let exHtml = '';
        if (data.examples.length > 0) {
            exHtml = `<div class="dict-example">"${_escHtml(data.examples[0])}"</div>`;
        }

        return `
            <div class="dict-result">
                <div class="dict-header">
                    <span class="dict-icon">📖</span>
                    <span class="dict-word">${_escHtml(data.word)}</span>
                    ${posTag}${pronTag}
                </div>
                ${defsHtml}${exHtml}${synHtml}
            </div>
        `;
    }

    _renderSuggestionHtml(data) {
        return `
            <div class="dict-result dict-suggestion">
                <span class="dict-icon">🔤</span>
                <span class="dict-suggest-text">Did you mean <strong>${_escHtml(data.word)}</strong>?</span>
            </div>
        `;
    }
}

function _escHtml(str) {
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

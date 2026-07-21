/**
 * Quick actions for selected text — smart chips that appear when
 * the floating window captures a text selection.
 */

import { detectScript, detectLanguageAll } from './language-detect.js';
import { COMMON_WORDS } from './quick-actions-language-data.js';

// --- Text classification ---

/**
 * Classify text content and return an array of content type tags.
 * Multiple tags can apply (e.g., code that contains an error).
 */
export function classifyText(text) {
    const types = [];
    if (looksLikeNumber(text)) types.push('number');
    if (looksLikeCode(text)) types.push('code');
    if (looksLikeError(text)) types.push('error');
    if (looksLikeJson(text)) types.push('json');
    if (looksLikeUrl(text)) types.push('url');
    if (looksLikeMath(text)) types.push('math');
    if (looksLikeFolderPlan(text)) types.push('folder_plan');
    if (types.length === 0) types.push('prose');
    return types;
}

function looksLikeNumber(text) {
    const trimmed = text.trim();
    // Pure number (with optional commas, decimals, negative, currency symbols)
    return /^[$€£¥]?\s*-?[\d,]+\.?\d*%?\s*$/.test(trimmed);
}

function looksLikeCode(text) {
    // Language keywords at start of line (broad set)
    const keywords =
        /^\s*(function|def|class|const|let|var|import|from|pub|fn|if|else|for|while|return|async|await|match|switch|case|try|catch|except|raise|throw|interface|struct|enum|impl|module|package|namespace|using|include|require|export|extends|implements)\b/m;
    if (keywords.test(text)) return true;

    // Keywords anywhere (for single-line pastes where newlines are collapsed)
    if (
        /\b(pub\s+fn|pub\s+async|async\s+fn|impl\s+\w+|struct\s+\w+|enum\s+\w+|trait\s+\w+)\b/.test(
            text
        )
    )
        return true;
    if (/\b(function\s+\w+|class\s+\w+|const\s+\w+\s*=|let\s+\w+\s*=|var\s+\w+\s*=)\b/.test(text))
        return true;
    if (/\b(def\s+\w+|import\s+\w+|from\s+\w+\s+import)\b/.test(text)) return true;

    // Rust/C-style attributes and annotations
    if (/#\[\w+/.test(text)) return true; // #[derive], #[tauri::command], etc.

    // Common code patterns
    if (/[{};]\s*$/m.test(text)) return true; // Lines ending with { } ;
    if (/\w+\(.*\)\s*[:{=>]/.test(text)) return true; // function calls followed by : { => (def foo(): / fn bar() { / x => )
    if (/\w+\(.*\)\s*$/.test(text) && /\bdef\b|\basync\b|\bfn\b|\bfunc\b/.test(text)) return true; // def/async/fn with parens
    if (/^\s*(\/\/|#!?|\/\*|\*\s|--\s)/m.test(text)) return true; // Comment lines
    if (/\.\w+\(/.test(text) && /[;{}():]/.test(text)) return true; // Method calls with code punctuation
    if (/\bself\b|\bthis\b/.test(text) && /[.()]/.test(text)) return true; // self.x or this.x patterns
    if (/=>\s*{/.test(text)) return true; // Arrow functions
    if (/\w+:\s*\w+\s*[,)]/.test(text) && /[(){}]/.test(text)) return true; // Type annotations like x: int, y: str
    if (/^\s*@\w+/m.test(text)) return true; // Decorators (@property, @app.route)
    if (/\bNone\b|\bnull\b|\bnil\b|\bundefined\b/.test(text) && /[=()]/.test(text)) return true; // None/null with assignment or call

    // High density of code punctuation (braces, semicolons, arrows, generics)
    const codePunctCount = (text.match(/[{};()[\]<>]/g) || []).length;
    if (codePunctCount >= 6 && codePunctCount / text.length > 0.03) return true;

    return false;
}

function looksLikeError(text) {
    if (/\b(Error|Exception|Traceback|FATAL|PANIC|panic)\b/i.test(text)) return true;
    if (/at\s+\S+:\d+:\d+/.test(text)) return true;
    if (/^\s+at\s+/m.test(text)) return true;
    if (/File ".*", line \d+/m.test(text)) return true;
    return false;
}

function looksLikeJson(text) {
    const trimmed = text.trim();
    if (
        (trimmed.startsWith('{') && trimmed.endsWith('}')) ||
        (trimmed.startsWith('[') && trimmed.endsWith(']'))
    ) {
        try {
            JSON.parse(trimmed);
            return true;
        } catch {}
    }
    // YAML-like (multiple key: value lines)
    if (/^\w+:\s+/m.test(trimmed) && /\n\w+:\s+/m.test(trimmed)) return true;
    return false;
}

function looksLikeUrl(text) {
    const trimmed = text.trim();
    return /^https?:\/\/\S+$/i.test(trimmed);
}

function looksLikeMath(text) {
    const trimmed = text.trim();
    if (!/\d/.test(trimmed)) return false;
    if (!/[+\-*/^%=]/.test(trimmed)) return false;
    if (/[a-z]{4,}\s+[a-z]{4,}/i.test(trimmed)) return false;
    return true;
}

function looksLikeFolderPlan(text) {
    // Detect folder organization responses — mentions of folders/files being organized,
    // plan proposals, or results of folder operations
    const _lower = text.toLowerCase();
    const hasOrgKeywords =
        /\b(organiz|folder|directory|move|moved|sorted|duplicat|clean|tidier)\b/i.test(text);
    const hasPlanIndicators =
        /\b(plan|operations?|completed|here'?s what|would you like|want me to)\b/i.test(text);
    return hasOrgKeywords && hasPlanIndicators;
}

// --- Built-in actions per content type ---

/**
 * Get the OS display language as a human-readable name.
 * Uses navigator.language (e.g., "en-US") and the Intl API to resolve it.
 */
function getOsLanguageName() {
    try {
        const locale = navigator.language || 'en';
        if (typeof Intl !== 'undefined' && Intl.DisplayNames) {
            const display = new Intl.DisplayNames(['en'], { type: 'language' });
            const name = display.of(locale);
            if (name) return name.charAt(0).toUpperCase() + name.slice(1);
        }
        // Fallback: map common locale prefixes
        const lang = locale.split('-')[0].toLowerCase();
        const map = {
            en: 'English',
            es: 'Spanish',
            fr: 'French',
            de: 'German',
            pt: 'Portuguese',
            it: 'Italian',
            ja: 'Japanese',
            ko: 'Korean',
            zh: 'Chinese',
            ru: 'Russian',
            ar: 'Arabic',
            hi: 'Hindi',
            nl: 'Dutch',
            sv: 'Swedish',
            pl: 'Polish',
            tr: 'Turkish',
            he: 'Hebrew',
        };
        return map[lang] || 'English';
    } catch {
        return 'English';
    }
}

/**
 * Get the OS language as an ISO 639-1 code (e.g. 'en', 'fr').
 */
function _getOsLanguageCode() {
    try {
        return (navigator.language || 'en').split('-')[0].toLowerCase();
    } catch {
        return 'en';
    }
}

/**
 * Strip markdown and code noise from text so language detection sees clean prose.
 * AI responses often contain code fences, inline code, URLs, and headings that
 * confuse statistical detectors.
 */
function _stripMarkdownForDetection(text) {
    return text
        .replace(/```[\s\S]*?```/g, ' ') // fenced code blocks
        .replace(/`[^`]*`/g, ' ') // inline code
        .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1') // markdown links/images → keep label
        .replace(/https?:\/\/\S+/g, ' ') // raw URLs
        .replace(/^\s*#{1,6}\s*/gm, '') // heading markers
        .replace(/^\s*[-*+]\s+/gm, '') // bullet markers
        .replace(/[*_~>]/g, ' ') // emphasis/quote punctuation
        .replace(/\s+/g, ' ')
        .trim();
}

/**
 * Very common short words per target language. If enough of these appear we can
 * declare the text is in that language without running statistical detection —
 * tinyld often mis-labels short text containing proper nouns, emojis, or slang.
 * Only languages that share the Latin alphabet need an entry here; scripts like
 * Cyrillic, Arabic, CJK, etc. are resolved via `detectScript` upstream.
 */
function _containsEnoughCommonWords(text, langCode, threshold = 3) {
    const list = COMMON_WORDS[langCode];
    if (!list) return false;
    const wordSet = new Set(list);
    const tokens = text.toLowerCase().match(/\b[\p{L}']+\b/gu) || [];
    if (tokens.length === 0) return false;
    let hits = 0;
    for (const tok of tokens) {
        if (wordSet.has(tok)) {
            hits++;
            if (hits >= threshold) return true;
        }
    }
    // For very short text, require proportionally fewer hits
    if (tokens.length < 10 && hits >= 2) return true;
    return false;
}

/**
 * Check if the text appears to be in the target language (or a variant of it).
 *
 * Returns true (text IS in target → hide translate chip) when:
 *   - The dominant Unicode script maps to the target language, or
 *   - Enough very common target-language words are present (short-circuit), or
 *   - Statistical detection identifies the target with reasonable confidence, or
 *   - Detection is too ambiguous to be trusted (default to "don't nag the user").
 *
 * Returns false (text is NOT in target → show translate chip) only when we're
 * confident the text is a different language.
 */
async function _isTextInTargetLanguage(text, targetLangCode) {
    // 1. Script detection (instant, definitive for non-Latin/Cyrillic scripts)
    const scriptLang = detectScript(text);
    if (scriptLang) return scriptLang === targetLangCode;

    // 2. Clean markdown noise so detection sees prose, not code fences
    const clean = _stripMarkdownForDetection(text);
    if (clean.length < 20) return true; // too little signal — don't nag

    // 3. Cheap common-word check — catches short/noisy text that trips tinyld
    if (_containsEnoughCommonWords(clean, targetLangCode)) return true;

    // 4. Statistical detection with confidence guardrails
    try {
        const candidates = await detectLanguageAll(clean);
        if (!candidates || candidates.length === 0) return true; // unclear — don't nag

        const top = candidates[0];
        if (top.lang === targetLangCode) return true;

        // Weak top guess, or target is a close runner-up → assume target.
        // tinyld tends to drift on short / mixed / markdown text.
        if (top.accuracy < 0.5) return true;
        const targetCandidate = candidates.find((c) => c.lang === targetLangCode);
        if (targetCandidate && top.accuracy - targetCandidate.accuracy < 0.2) return true;

        return false;
    } catch {
        return true; // detection failed — default to hiding the chip
    }
}

// Minimum word count for the "Summarize" action to be useful — short snippets
// don't benefit from summarization.
const SUMMARIZE_MIN_WORDS = 40;

function countWords(text) {
    return (text.trim().match(/\S+/g) || []).length;
}

const BUILTIN_ACTIONS = [
    // Universal — only shown when text is long enough to benefit from summarization
    {
        label: 'Summarize',
        icon: '📝',
        prompt: 'Summarize the following text concisely:\n\n{text}',
        contentTypes: [],
        mode: 'inform',
        minWords: SUMMARIZE_MIN_WORDS,
    },
    // Prose
    {
        label: 'Translate',
        icon: '🌐',
        prompt: null,
        contentTypes: ['prose'],
        _dynamic: 'translate',
        mode: 'replace',
    },
    // Code
    {
        label: 'Explain',
        icon: '💡',
        prompt: 'Explain what this code does in plain language:\n\n```\n{text}\n```',
        contentTypes: ['code'],
        mode: 'inform',
    },
    {
        label: 'Add comments',
        icon: '💬',
        prompt: 'Add clear, helpful comments to this code. Return only the commented code, no explanations:\n\n```\n{text}\n```',
        contentTypes: ['code'],
        mode: 'replace',
    },
    {
        label: 'Find bugs',
        icon: '🐛',
        prompt: 'Review this code for bugs, issues, or improvements:\n\n```\n{text}\n```',
        contentTypes: ['code'],
        mode: 'inform',
    },
    // Errors
    {
        label: 'Explain error',
        icon: '🔍',
        prompt: 'Explain this error and suggest how to fix it:\n\n```\n{text}\n```',
        contentTypes: ['error'],
        mode: 'inform',
    },
    {
        label: 'Suggest fix',
        icon: '🔧',
        prompt: 'Suggest a fix for this error. Return only the corrected code:\n\n```\n{text}\n```',
        contentTypes: ['error'],
        mode: 'replace',
    },
    // JSON/data
    {
        label: 'Format',
        icon: '📐',
        prompt: 'Format and pretty-print this data. Return only the formatted data:\n\n```\n{text}\n```',
        contentTypes: ['json'],
        mode: 'replace',
    },
    {
        label: 'Validate',
        icon: '✅',
        prompt: 'Validate this data structure and point out any issues:\n\n```\n{text}\n```',
        contentTypes: ['json'],
        mode: 'inform',
    },
    // URL
    {
        label: 'Summarize page',
        icon: '🌐',
        prompt: 'Summarize the content at this URL:\n\n{text}',
        contentTypes: ['url'],
        mode: 'inform',
    },
    // Number
    {
        label: 'Convert units',
        icon: '📏',
        prompt: 'What are common unit conversions for this number? Show conversions for likely units (currency, distance, weight, temperature, etc.):\n\n{text}',
        contentTypes: ['number'],
        mode: 'inform',
    },
    {
        label: 'Explain number',
        icon: '🔢',
        prompt: 'What is significant about this number? Provide context (is it a port number, HTTP status, error code, mathematical constant, etc.):\n\n{text}',
        contentTypes: ['number'],
        mode: 'inform',
    },
    // Folder organization
    {
        label: 'Looks good, do it',
        icon: '▶️',
        prompt: 'Go ahead and execute the plan as proposed.',
        contentTypes: ['folder_plan'],
        mode: 'inform',
    },
    {
        label: 'More details',
        icon: '🔍',
        prompt: 'Can you give me more details about what each operation will do and why?',
        contentTypes: ['folder_plan'],
        mode: 'inform',
    },
    {
        label: 'Undo changes',
        icon: '↩️',
        prompt: 'Please undo/rollback the folder changes that were just made.',
        contentTypes: ['folder_plan'],
        mode: 'inform',
    },
];

// --- Chip rendering ---

/**
 * Get the list of actions to show for the given text.
 * Merges built-in smart actions with user-configured custom actions.
 * @param {string} text - The selected text
 * @param {object} config - The quick_actions config { enabled, custom_actions }
 * @returns {Array<{ label, icon, prompt }>}
 */
export async function getActionsForText(text, config) {
    if (!config?.enabled) return [];

    const types = classifyText(text);
    const actions = [];
    const translateLang = config.translate_language || getOsLanguageName();
    const wordCount = countWords(text);

    // Built-in actions: include if contentTypes is empty (universal) or matches
    for (const action of BUILTIN_ACTIONS) {
        // Respect per-action minimum word count (e.g. Summarize needs enough content)
        if (action.minWords && wordCount < action.minWords) continue;

        if (
            action.contentTypes.length === 0 ||
            action.contentTypes.some((t) => types.includes(t))
        ) {
            // Handle dynamic translate action — only show if text is in a different language
            if (action._dynamic === 'translate') {
                const targetLangCode = _getOsLanguageCode();
                if (!(await _isTextInTargetLanguage(text, targetLangCode))) {
                    actions.push({
                        ...action,
                        label: `→ ${translateLang}`,
                        prompt: `Translate the following text to ${translateLang}. Return only the translated text:\n\n{text}`,
                    });
                }
            } else {
                actions.push(action);
            }
        }
    }

    // Custom actions from config
    if (config.custom_actions) {
        for (const custom of config.custom_actions) {
            if (
                !custom.content_types ||
                custom.content_types.length === 0 ||
                custom.content_types.some((t) => types.includes(t))
            ) {
                actions.push(custom);
            }
        }
    }

    return actions;
}

/**
 * Render quick action chips into a container element.
 * @param {Array} actions - From getActionsForText
 * @param {HTMLElement} container - Element to render into
 * @param {function} onAction - Callback(prompt) when a chip is clicked
 */
export function renderQuickActionChips(actions, container, onAction) {
    container.innerHTML = '';
    if (!actions.length) {
        container.style.display = 'none';
        return;
    }
    container.style.display = 'flex';
    for (const action of actions) {
        const chip = document.createElement('button');
        chip.className = 'quick-action-chip';
        chip.title = action.label;
        const iconSpan = document.createElement('span');
        iconSpan.className = 'quick-action-icon';
        iconSpan.textContent = action.icon || '⚡';
        const labelSpan = document.createElement('span');
        labelSpan.className = 'quick-action-label';
        labelSpan.textContent = action.label;
        chip.appendChild(iconSpan);
        chip.appendChild(labelSpan);
        chip.addEventListener('click', () => {
            // Telemetry: chip-click is the only meaningful "user picked
            // this action" signal. Built-in actions have fixed labels
            // from getActionsForText (Translate / Summarize / Explain /
            // etc.); custom actions carry user-typed labels. Send the
            // built-in label verbatim and bucket custom ones to a
            // single name so we can see the split without leaking
            // user-typed copy. Lazy-import keeps this module loadable
            // in non-Tauri test contexts.
            const KNOWN = new Set([
                'Translate',
                'Summarize',
                'Explain',
                'Rewrite',
                'Fix grammar',
                'Make formal',
                'Make casual',
                'Code review',
                'Explain code',
                'Add comments',
            ]);
            const label = KNOWN.has(action.label) ? action.label : 'custom';
            import('./telemetry.js')
                .then(({ trackEvent }) => {
                    trackEvent('quick_action_used', { action: label });
                })
                .catch(() => {});
            onAction(action.prompt);
        });
        container.appendChild(chip);
    }
}

/**
 * Spelling suggestions in Kage's own right-click menu. The menu is a
 * separate popup window (popup-menu.js), not the webview's native menu, so
 * it can't show WebKit's spelling items; instead the caller finds the word
 * that was right-clicked, asks the backend (`spell_suggestions`, the OS
 * spell checker) about it, and puts the guesses at the top of the menu.
 *
 * Finding the word: on macOS, WebKit selects the word under the pointer
 * before `contextmenu` fires, so the input's selection already covers it.
 * Elsewhere (WebView2) the selection may be collapsed, so the word is
 * expanded from the right-click point (`caretPositionFromPoint`, when it
 * answers with the input itself) or, failing that, from the caret.
 */

import { SEPARATOR, defaultTextMenuItems } from './popup-menu.js';

/** Most suggestions shown (the backend sends no more either). */
export const MAX_SPELL_SUGGESTIONS = 5;
/** Longest word asked about, in characters (the backend's limit too). */
export const MAX_SPELL_WORD_CHARS = 64;
/** How long the menu waits for suggestions before showing without them. */
export const SPELL_LOOKUP_TIMEOUT_MS = 150;

/** Menu item ids. Suggestion `i` is `spell:<i>`. */
export const SPELL_LEARN_ID = 'spell:learn';
export const SPELL_NONE_ID = 'spell:none';
const SUGGESTION_ID_RE = /^spell:(\d+)$/;

/** Labels the backend would cut anyway (MAX_LABEL_CHARS). */
const MAX_LABEL_CHARS = 200;

/** A run of word characters: letters, combining marks, apostrophes. */
const WORD_RUN_RE = /[\p{L}\p{M}'’]+/gu;
const WORD_ONLY_RE = /^[\p{L}\p{M}'’]+$/u;
const LETTER_RE = /\p{L}/u;
const APOSTROPHE_RE = /^['’]$/;
const SPACE_RE = /^\s$/;

/**
 * `{ start, end, word }` for `text[start, end)` once leading and trailing
 * apostrophes (quotes) are dropped; `null` if nothing word-like is left or
 * it's too long to check.
 */
function wordRange(text, start, end) {
    while (start < end && APOSTROPHE_RE.test(text[start])) start++;
    while (end > start && APOSTROPHE_RE.test(text[end - 1])) end--;
    const word = text.slice(start, end);
    if (!word || !LETTER_RE.test(word) || [...word].length > MAX_SPELL_WORD_CHARS) return null;
    return { start, end, word };
}

/**
 * The word containing (or touching) UTF-16 `offset` in `text`, as
 * `{ start, end, word }`; `null` when `offset` sits between words.
 */
export function wordAt(text, offset) {
    if (typeof text !== 'string' || !Number.isInteger(offset)) return null;
    if (offset < 0 || offset > text.length) return null;
    for (const match of text.matchAll(WORD_RUN_RE)) {
        const start = match.index;
        if (start > offset) break;
        const end = start + match[0].length;
        if (offset <= end) return wordRange(text, start, end);
    }
    return null;
}

/**
 * The selection `text[start, end)` as a word range, if it is exactly one
 * word (surrounding whitespace aside — a Windows double-click takes the
 * trailing space too); else `null`.
 */
export function wordInSelection(text, start, end) {
    if (typeof text !== 'string' || !Number.isInteger(start) || !Number.isInteger(end)) {
        return null;
    }
    if (start > end) [start, end] = [end, start];
    start = Math.max(0, start);
    end = Math.min(text.length, end);
    while (start < end && SPACE_RE.test(text[start])) start++;
    while (end > start && SPACE_RE.test(text[end - 1])) end--;
    if (start === end || !WORD_ONLY_RE.test(text.slice(start, end))) return null;
    return wordRange(text, start, end);
}

/**
 * The word a right-click in a text field is about, as `{ start, end, word }`
 * (offsets into `text`), or `null`.
 *
 * - A selection that holds the point (or with no point known) counts only
 *   if it is a single word.
 * - Otherwise the word at `pointOffset` (where the click landed), else the
 *   word at the caret.
 */
export function findSpellWord({ text, selectionStart, selectionEnd, pointOffset = null }) {
    if (typeof text !== 'string' || !text) return null;
    const start = Number.isInteger(selectionStart) ? selectionStart : null;
    const end = Number.isInteger(selectionEnd) ? selectionEnd : start;
    const point = Number.isInteger(pointOffset) ? pointOffset : null;
    if (start !== null && start !== end) {
        const lo = Math.min(start, end);
        const hi = Math.max(start, end);
        if (point === null || (point >= lo && point <= hi)) return wordInSelection(text, lo, hi);
    }
    const offset = point ?? start;
    return offset === null ? null : wordAt(text, offset);
}

/**
 * The offset into `input.value` under client point (x, y), when the
 * browser can say (`caretPositionFromPoint` answering with the input
 * itself); else `null`.
 */
export function inputOffsetAtPoint(input, x, y) {
    const doc = input?.ownerDocument;
    if (typeof doc?.caretPositionFromPoint !== 'function') return null;
    try {
        const pos = doc.caretPositionFromPoint(x, y);
        if (pos && pos.offsetNode === input && Number.isInteger(pos.offset)) return pos.offset;
    } catch {}
    return null;
}

/** Usable suggestions: trimmed, non-empty, unique, at most five. */
export function cleanSuggestions(suggestions) {
    const out = [];
    for (const s of Array.isArray(suggestions) ? suggestions : []) {
        if (typeof s !== 'string') continue;
        const label = s.trim();
        if (!label || label.length > MAX_LABEL_CHARS || out.includes(label)) continue;
        out.push(label);
        if (out.length === MAX_SPELL_SUGGESTIONS) break;
    }
    return out;
}

/**
 * Menu items for a misspelled word: its suggestions (or a disabled "No
 * guesses found"), Learn spelling, then the usual text menu. Suggestions
 * are the user's language, so they're plain labels, never i18n keys.
 */
export function spellMenuItems(suggestions, { devMode = false } = {}) {
    const guesses = cleanSuggestions(suggestions);
    const head = guesses.length
        ? guesses.map((label, i) => ({ id: `spell:${i}`, label }))
        : [{ id: SPELL_NONE_ID, labelKey: 'context_menu.no_guesses', enabled: false }];
    return [
        ...head,
        SEPARATOR,
        { id: SPELL_LEARN_ID, labelKey: 'context_menu.learn_spelling' },
        SEPARATOR,
        ...defaultTextMenuItems({ devMode }),
    ];
}

/**
 * What a menu choice means for the spelling items: `{ replace: text }`,
 * `{ learn: true }`, or `null` (not a spelling item — a text-menu action
 * or a dismissal). `suggestions` is what the menu was built from.
 */
export function spellChoice(action, suggestions) {
    if (action === SPELL_LEARN_ID) return { learn: true };
    const m = typeof action === 'string' ? SUGGESTION_ID_RE.exec(action) : null;
    if (!m) return null;
    const text = cleanSuggestions(suggestions)[Number(m[1])];
    return text === undefined ? null : { replace: text };
}

/**
 * Replace `range` (from `findSpellWord`) in `input` with `text`, as typing
 * would: `insertText`, so it's one undo step and `input` fires. Falls back
 * to editing the value (and firing `input`) where `execCommand` doesn't
 * apply. `false` if the text under `range` is no longer the word.
 */
export function replaceWord(input, range, text) {
    if (!input || !range || typeof text !== 'string') return false;
    const before = input.value;
    if (before.slice(range.start, range.end) !== range.word) return false;
    input.focus();
    input.setSelectionRange(range.start, range.end);
    try {
        input.ownerDocument.execCommand('insertText', false, text);
    } catch {}
    // Applied (or had nothing to change): done. Changed some other way:
    // leave it rather than apply the replacement twice.
    if (input.value !== before || text === range.word) return true;
    input.value = before.slice(0, range.start) + text + before.slice(range.end);
    const caret = range.start + text.length;
    input.setSelectionRange(caret, caret);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    return true;
}

/** `promise`'s value, or `fallback` if it rejects or takes over `ms`. */
export function withTimeout(promise, ms, fallback = null) {
    return new Promise((resolve) => {
        const timer = setTimeout(() => resolve(fallback), ms);
        Promise.resolve(promise).then(
            (value) => {
                clearTimeout(timer);
                resolve(value);
            },
            () => {
                clearTimeout(timer);
                resolve(fallback);
            }
        );
    });
}

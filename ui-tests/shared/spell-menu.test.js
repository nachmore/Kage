/**
 * Spelling items in Kage's right-click menu: finding the right-clicked
 * word in a text field, building the menu, mapping a choice back, and
 * replacing the word.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import {
    MAX_SPELL_SUGGESTIONS,
    MAX_SPELL_WORD_CHARS,
    SPELL_LEARN_ID,
    SPELL_NONE_ID,
    cleanSuggestions,
    findSpellWord,
    inputOffsetAtPoint,
    replaceWord,
    spellChoice,
    spellMenuItems,
    wordAt,
    wordInSelection,
    withTimeout,
} from '../../ui/js/shared/spell-menu.js';
import { defaultTextMenuItems, normalizeMenuItems } from '../../ui/js/shared/popup-menu.js';

const range = (text, word, from = 0) => {
    const start = text.indexOf(word, from);
    return { start, end: start + word.length, word };
};

describe('wordAt', () => {
    const text = 'say helloo there';

    it('finds the word around, at the start of, or at the end of the offset', () => {
        expect(wordAt(text, 6)).toEqual(range(text, 'helloo'));
        expect(wordAt(text, 4)).toEqual(range(text, 'helloo'));
        expect(wordAt(text, 10)).toEqual(range(text, 'helloo'));
        expect(wordAt(text, 0)).toEqual(range(text, 'say'));
        expect(wordAt(text, text.length)).toEqual(range(text, 'there'));
    });

    it('is null between words and for bad offsets', () => {
        expect(wordAt('a  b', 2)).toBeNull();
        expect(wordAt('helloo ', 7)).toBeNull();
        expect(wordAt(text, -1)).toBeNull();
        expect(wordAt(text, text.length + 1)).toBeNull();
        expect(wordAt(text, 1.5)).toBeNull();
        expect(wordAt(null, 0)).toBeNull();
    });

    it('keeps inner apostrophes and drops quotes around the word', () => {
        expect(wordAt("I don't know", 4)).toEqual(range("I don't know", "don't"));
        expect(wordAt('it’s fine', 1)).toEqual(range('it’s fine', 'it’s'));
        const quoted = "say 'helloo' now";
        expect(wordAt(quoted, 6)).toEqual(range(quoted, 'helloo'));
        expect(wordAt("a ' b", 2)).toBeNull();
    });

    it('stops at digits and punctuation', () => {
        const text2 = 'abc123def, ghi-jkl';
        expect(wordAt(text2, 1)).toEqual(range(text2, 'abc'));
        expect(wordAt(text2, 4)).toBeNull();
        expect(wordAt(text2, 7)).toEqual(range(text2, 'def'));
        expect(wordAt(text2, 12)).toEqual(range(text2, 'ghi'));
        expect(wordAt(text2, 16)).toEqual(range(text2, 'jkl'));
    });

    it('handles any script, combining marks and astral letters', () => {
        const de = 'Grüße an alle';
        expect(wordAt(de, 2)).toEqual(range(de, 'Grüße'));
        const decomposed = 'naïve x';
        expect(wordAt(decomposed, 1)).toEqual(range(decomposed, 'naïve'));
        const ru = 'привет мир';
        expect(wordAt(ru, 8)).toEqual(range(ru, 'мир'));
        // U+1D400 MATHEMATICAL BOLD CAPITAL A is a letter outside the BMP.
        const astral = 'x \u{1D400}\u{1D401} y';
        expect(wordAt(astral, 3)).toEqual(range(astral, '\u{1D400}\u{1D401}'));
    });

    it('skips words too long to check', () => {
        const long = 'a'.repeat(MAX_SPELL_WORD_CHARS + 1);
        expect(wordAt(long, 3)).toBeNull();
        const ok = 'a'.repeat(MAX_SPELL_WORD_CHARS);
        expect(wordAt(ok, 3)?.word).toBe(ok);
    });
});

describe('wordInSelection', () => {
    const text = 'say helloo there';

    it('accepts exactly one selected word, ignoring surrounding spaces', () => {
        const { start, end } = range(text, 'helloo');
        expect(wordInSelection(text, start, end)).toEqual(range(text, 'helloo'));
        expect(wordInSelection(text, start, end + 1)).toEqual(range(text, 'helloo'));
        expect(wordInSelection(text, start - 1, end + 1)).toEqual(range(text, 'helloo'));
        expect(wordInSelection(text, end, start)).toEqual(range(text, 'helloo'));
    });

    it('rejects several words, part-punctuation and empty selections', () => {
        expect(wordInSelection(text, 0, 10)).toBeNull();
        expect(wordInSelection('hi, there', 0, 3)).toBeNull();
        expect(wordInSelection(text, 3, 4)).toBeNull();
        expect(wordInSelection(text, 4, 4)).toBeNull();
    });
});

describe('findSpellWord', () => {
    const text = 'say helloo there';
    const hello = range(text, 'helloo');
    const there = range(text, 'there');

    it('uses a one-word selection (macOS selects the word on right-click)', () => {
        expect(
            findSpellWord({ text, selectionStart: hello.start, selectionEnd: hello.end })
        ).toEqual(hello);
        expect(
            findSpellWord({
                text,
                selectionStart: hello.start,
                selectionEnd: hello.end,
                pointOffset: hello.start + 2,
            })
        ).toEqual(hello);
    });

    it('offers nothing for a multi-word selection under the pointer', () => {
        expect(findSpellWord({ text, selectionStart: 0, selectionEnd: 10 })).toBeNull();
        expect(
            findSpellWord({ text, selectionStart: 0, selectionEnd: 10, pointOffset: 5 })
        ).toBeNull();
    });

    it('prefers the clicked word over a selection elsewhere', () => {
        expect(
            findSpellWord({
                text,
                selectionStart: hello.start,
                selectionEnd: hello.end,
                pointOffset: there.start + 1,
            })
        ).toEqual(there);
    });

    it('expands a collapsed selection from the click point, else the caret', () => {
        expect(
            findSpellWord({ text, selectionStart: 2, selectionEnd: 2, pointOffset: 13 })
        ).toEqual(there);
        expect(findSpellWord({ text, selectionStart: 7, selectionEnd: 7 })).toEqual(hello);
        expect(
            findSpellWord({ text, selectionStart: 7, selectionEnd: 7, pointOffset: null })
        ).toEqual(hello);
        // The click point wins over the caret; a click between words
        // doesn't fall back to the caret's word.
        expect(
            findSpellWord({ text, selectionStart: 7, selectionEnd: 7, pointOffset: 3 })
        ).toEqual(range(text, 'say'));
        expect(
            findSpellWord({ text: 'a  b', selectionStart: 0, selectionEnd: 0, pointOffset: 2 })
        ).toBeNull();
    });

    it('is null for empty or missing text', () => {
        expect(findSpellWord({ text: '', selectionStart: 0, selectionEnd: 0 })).toBeNull();
        expect(findSpellWord({ text: undefined, selectionStart: 0, selectionEnd: 0 })).toBeNull();
        expect(findSpellWord({ text: 'abc' })).toBeNull();
    });
});

describe('inputOffsetAtPoint', () => {
    const fakeInput = (caretPositionFromPoint) => {
        const input = { ownerDocument: { caretPositionFromPoint } };
        return input;
    };

    it('answers only when the caret position is in the input itself', () => {
        const input = fakeInput(null);
        input.ownerDocument.caretPositionFromPoint = () => ({ offsetNode: input, offset: 4 });
        expect(inputOffsetAtPoint(input, 1, 2)).toBe(4);
        const other = fakeInput(() => ({ offsetNode: {}, offset: 4 }));
        expect(inputOffsetAtPoint(other, 1, 2)).toBeNull();
        expect(inputOffsetAtPoint(fakeInput(() => null), 1, 2)).toBeNull();
    });

    it('is null without the API, or when it throws', () => {
        expect(inputOffsetAtPoint(fakeInput(undefined), 1, 2)).toBeNull();
        expect(
            inputOffsetAtPoint(
                fakeInput(() => {
                    throw new Error('nope');
                }),
                1,
                2
            )
        ).toBeNull();
        expect(inputOffsetAtPoint(null, 1, 2)).toBeNull();
    });
});

describe('cleanSuggestions', () => {
    it('trims, dedupes, drops junk and caps at five', () => {
        expect(
            cleanSuggestions([' hello ', 'hello', '', 7, null, 'hell', 'halo', 'hallo', 'hullo', 'x'])
        ).toEqual(['hello', 'hell', 'halo', 'hallo', 'hullo']);
        expect(cleanSuggestions(['x'.repeat(201), 'ok'])).toEqual(['ok']);
        expect(cleanSuggestions(undefined)).toEqual([]);
        expect(MAX_SPELL_SUGGESTIONS).toBe(5);
    });
});

describe('spellMenuItems', () => {
    const textMenu = defaultTextMenuItems();

    it('lists suggestions as plain labels, then Learn, then the text menu', () => {
        const items = spellMenuItems(['hello', 'hellos']);
        expect(items).toEqual([
            { id: 'spell:0', label: 'hello' },
            { id: 'spell:1', label: 'hellos' },
            { separator: true },
            { id: SPELL_LEARN_ID, labelKey: 'context_menu.learn_spelling' },
            { separator: true },
            ...textMenu,
        ]);
    });

    it('shows a disabled "No guesses found" when there are none', () => {
        const [first, sep, learn] = spellMenuItems([]);
        expect(first).toEqual({
            id: SPELL_NONE_ID,
            labelKey: 'context_menu.no_guesses',
            enabled: false,
        });
        expect(sep).toEqual({ separator: true });
        expect(learn.id).toBe(SPELL_LEARN_ID);
    });

    it('includes Inspect in dev mode, exactly as the default menu does', () => {
        const items = spellMenuItems(['hello'], { devMode: true });
        expect(items.slice(4)).toEqual(defaultTextMenuItems({ devMode: true }));
        expect(items.at(-1).id).toBe('inspect');
        expect(spellMenuItems(['hello']).some((i) => i.id === 'inspect')).toBe(false);
    });

    it('caps suggestions at five', () => {
        const items = spellMenuItems(['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        expect(items.filter((i) => /^spell:\d+$/.test(i.id ?? ''))).toHaveLength(5);
    });

    it('survives the popup menu normalisation unchanged in order and ids', () => {
        const items = spellMenuItems(['héllo wörld', "don't", '<b>x</b>'], { devMode: true });
        const normalized = normalizeMenuItems(items);
        expect(normalized.map((i) => i.id ?? 'sep')).toEqual(
            items.map((i) => i.id ?? 'sep')
        );
        expect(normalized.slice(0, 3).map((i) => i.label)).toEqual([
            'héllo wörld',
            "don't",
            '<b>x</b>',
        ]);
        expect(normalized[0].labelKey).toBeUndefined();
        const none = normalizeMenuItems(spellMenuItems([]))[0];
        expect(none).toMatchObject({ id: SPELL_NONE_ID, enabled: false });
    });
});

describe('spellChoice', () => {
    const suggestions = ['hello', 'hellos'];

    it('maps suggestion ids back to their text', () => {
        expect(spellChoice('spell:0', suggestions)).toEqual({ replace: 'hello' });
        expect(spellChoice('spell:1', suggestions)).toEqual({ replace: 'hellos' });
    });

    it('recognises Learn spelling', () => {
        expect(spellChoice(SPELL_LEARN_ID, suggestions)).toEqual({ learn: true });
    });

    it('is null for text-menu actions, dismissals and unknown indices', () => {
        for (const action of ['cut', 'paste', 'dismissed', SPELL_NONE_ID, 'spell:9', 'spell:-1']) {
            expect(spellChoice(action, suggestions)).toBeNull();
        }
        expect(spellChoice(undefined, suggestions)).toBeNull();
    });

    it('indexes the same cleaned list the menu was built from', () => {
        const raw = [' hello ', 'hello', 'hellos'];
        const items = spellMenuItems(raw);
        expect(items[1]).toEqual({ id: 'spell:1', label: 'hellos' });
        expect(spellChoice('spell:1', raw)).toEqual({ replace: 'hellos' });
    });
});

describe('replaceWord', () => {
    afterEach(() => {
        document.body.innerHTML = '';
        vi.restoreAllMocks();
    });

    const makeInput = (value) => {
        const input = document.createElement('textarea');
        input.value = value;
        document.body.appendChild(input);
        return input;
    };

    it('replaces the word with insertText when the browser supports it', () => {
        const input = makeInput('say helloo there');
        const r = range(input.value, 'helloo');
        document.execCommand = vi.fn((cmd, _ui, text) => {
            expect(cmd).toBe('insertText');
            expect([input.selectionStart, input.selectionEnd]).toEqual([r.start, r.end]);
            input.setRangeText(text, input.selectionStart, input.selectionEnd, 'end');
            return true;
        });
        const onInput = vi.fn();
        input.addEventListener('input', onInput);
        expect(replaceWord(input, r, 'hello')).toBe(true);
        expect(input.value).toBe('say hello there');
        expect(document.execCommand).toHaveBeenCalledTimes(1);
        // The browser fires `input` itself for insertText; no second one.
        expect(onInput).not.toHaveBeenCalled();
        expect(document.activeElement).toBe(input);
    });

    it('falls back to editing the value and firing input', () => {
        const input = makeInput('say helloo there');
        document.execCommand = vi.fn(() => false);
        const onInput = vi.fn();
        input.addEventListener('input', onInput);
        expect(replaceWord(input, range(input.value, 'helloo'), 'hello')).toBe(true);
        expect(input.value).toBe('say hello there');
        expect(onInput).toHaveBeenCalledTimes(1);
        expect([input.selectionStart, input.selectionEnd]).toEqual([9, 9]);
    });

    it('also falls back when execCommand throws', () => {
        const input = makeInput('helloo');
        document.execCommand = vi.fn(() => {
            throw new Error('unsupported');
        });
        expect(replaceWord(input, range(input.value, 'helloo'), 'hello')).toBe(true);
        expect(input.value).toBe('hello');
    });

    it('leaves the text alone if the word is no longer there', () => {
        const input = makeInput('say helloo there');
        const r = range(input.value, 'helloo');
        input.value = 'something else';
        document.execCommand = vi.fn(() => true);
        expect(replaceWord(input, r, 'hello')).toBe(false);
        expect(input.value).toBe('something else');
        expect(document.execCommand).not.toHaveBeenCalled();
    });

    it('re-inserting the same word (after Learn) never duplicates it', () => {
        const input = makeInput('say helloo there');
        document.execCommand = vi.fn(() => false);
        const onInput = vi.fn();
        input.addEventListener('input', onInput);
        expect(replaceWord(input, range(input.value, 'helloo'), 'helloo')).toBe(true);
        expect(input.value).toBe('say helloo there');
        expect(onInput).not.toHaveBeenCalled();
    });
});

describe('withTimeout', () => {
    it('resolves with the value when it arrives in time', async () => {
        await expect(withTimeout(Promise.resolve(['a']), 50, null)).resolves.toEqual(['a']);
    });

    it('resolves with the fallback on rejection', async () => {
        await expect(withTimeout(Promise.reject(new Error('x')), 50, 'fb')).resolves.toBe('fb');
    });

    it('resolves with the fallback when the value is late', async () => {
        vi.useFakeTimers();
        try {
            let resolveLate;
            const late = new Promise((r) => {
                resolveLate = r;
            });
            const raced = withTimeout(late, 150, null);
            vi.advanceTimersByTime(151);
            resolveLate(['too late']);
            await expect(raced).resolves.toBeNull();
        } finally {
            vi.useRealTimers();
        }
    });
});

import { describe, it, expect } from 'vitest';
import {
    validateSchema,
    sanitizeInfoHtml,
    defaultValues,
    isVisible,
    copyTextError,
    isScratchId,
    MAX_COPY_LENGTH,
} from '../../ui/js/shared/settings-schema.js';

describe('validateSchema', () => {
    it('accepts a minimal empty schema', () => {
        const r = validateSchema({ sections: [] });
        expect(r.ok).toBe(true);
    });

    it('rejects non-objects', () => {
        expect(validateSchema(null).ok).toBe(false);
        expect(validateSchema('x').ok).toBe(false);
        expect(validateSchema([]).ok).toBe(false);
    });

    it('rejects missing sections', () => {
        const r = validateSchema({});
        expect(r.ok).toBe(false);
        expect(r.error).toMatch(/sections/);
    });

    it('rejects duplicate control ids', () => {
        const r = validateSchema({
            sections: [
                { controls: [{ type: 'text', id: 'a', label: 'A' }] },
                { controls: [{ type: 'text', id: 'a', label: 'A again' }] },
            ],
        });
        expect(r.ok).toBe(false);
        expect(r.error).toMatch(/duplicated/);
    });

    it('rejects invalid control types', () => {
        const r = validateSchema({
            sections: [{ controls: [{ type: 'banana', id: 'x', label: 'x' }] }],
        });
        expect(r.ok).toBe(false);
        expect(r.error).toMatch(/type/);
    });

    it('rejects ids with unsafe characters', () => {
        const r = validateSchema({
            sections: [{ controls: [{ type: 'text', id: '../evil', label: 'x' }] }],
        });
        expect(r.ok).toBe(false);
    });

    it('accepts a select with options', () => {
        const r = validateSchema({
            sections: [{
                controls: [{
                    type: 'select', id: 'mode', label: 'Mode',
                    options: [{ value: 'a', label: 'A' }],
                }],
            }],
        });
        expect(r.ok).toBe(true);
    });

    it('rejects a select with no options', () => {
        const r = validateSchema({
            sections: [{
                controls: [{ type: 'select', id: 'mode', label: 'Mode', options: [] }],
            }],
        });
        expect(r.ok).toBe(false);
    });

    it('rejects a range with inverted bounds', () => {
        const r = validateSchema({
            sections: [{
                controls: [{ type: 'range', id: 'r', label: 'R', min: 10, max: 5 }],
            }],
        });
        expect(r.ok).toBe(false);
    });

    it('rejects an action with no action name', () => {
        const r = validateSchema({
            sections: [{
                controls: [{ type: 'action', id: 'go', label: 'Go', action: '' }],
            }],
        });
        expect(r.ok).toBe(false);
    });
});

describe('validateSchema — copy actions', () => {
    const one = (ctrl, ...rest) => validateSchema({ sections: [{ controls: [ctrl, ...rest] }] });
    const copyAction = (copy, extra = {}) => ({ type: 'action', id: 'cp', label: 'Copy', copy, ...extra });

    it('accepts a literal copy value without an RPC name', () => {
        expect(one(copyAction({ value: 'http://127.0.0.1:43821/spotify/callback' })).ok).toBe(true);
    });

    it('accepts a copy action that keeps an RPC fallback for older hosts', () => {
        expect(one(copyAction({ value: 'x' }, { action: 'copy_fallback' })).ok).toBe(true);
    });

    it('still rejects an empty RPC name when one is given', () => {
        expect(one(copyAction({ value: 'x' }, { action: '' })).ok).toBe(false);
    });

    it('accepts copy.from pointing at a text / number / select control, even later in the schema', () => {
        const r = validateSchema({
            sections: [
                { controls: [copyAction({ from: 'client_id' })] },
                { controls: [{ type: 'text', id: 'client_id', label: 'Client ID' }] },
            ],
        });
        expect(r.ok).toBe(true);
        expect(one(copyAction({ from: 'n' }), { type: 'number', id: 'n', label: 'N' }).ok).toBe(true);
        expect(one(copyAction({ from: 's' }), {
            type: 'select', id: 's', label: 'S', options: [{ value: 'a', label: 'A' }],
        }).ok).toBe(true);
    });

    it('rejects copy.from naming a missing or non-text control', () => {
        expect(one(copyAction({ from: 'nope' })).error).toMatch(/copy\.from/);
        expect(one(copyAction({ from: 'flag' }), { type: 'checkbox', id: 'flag', label: 'F' }).ok).toBe(false);
        expect(one(copyAction({ from: 'cp' })).ok).toBe(false); // itself (an action)
        expect(one(copyAction({ from: '../x' })).ok).toBe(false);
    });

    it('needs exactly one of value / from', () => {
        expect(one(copyAction({})).error).toMatch(/exactly one/);
        expect(one(copyAction({ value: 'a', from: 'b' })).error).toMatch(/exactly one/);
        expect(one(copyAction('text')).error).toMatch(/copy must be an object/);
        expect(one(copyAction(['x'])).ok).toBe(false);
    });

    it('rejects non-string, empty, oversized, or rich copy values', () => {
        expect(one(copyAction({ value: 42 })).error).toMatch(/must be a string/);
        expect(one(copyAction({ value: { html: '<b>x</b>' } })).error).toMatch(/must be a string/);
        expect(one(copyAction({ value: '' })).error).toMatch(/must not be empty/);
        expect(one(copyAction({ value: 'x'.repeat(MAX_COPY_LENGTH) })).ok).toBe(true);
        expect(one(copyAction({ value: 'x'.repeat(MAX_COPY_LENGTH + 1) })).error).toMatch(/at most/);
        // Terminal escape sequence (pastejacking) and a bidi override.
        expect(one(copyAction({ value: 'ls\u001b[2J' })).error).toMatch(/plain text/);
        expect(one(copyAction({ value: 'abc\u202edef' })).error).toMatch(/plain text/);
    });

    it('copyTextError allows tabs and newlines', () => {
        expect(copyTextError('line one\nline two\tend\r\n')).toBeNull();
        expect(copyTextError('\u0000')).not.toBeNull();
        expect(copyTextError('\u0085')).not.toBeNull();
    });
});

describe('isScratchId', () => {
    it('is true only for ids starting with a double underscore', () => {
        expect(isScratchId('__setup_client_id')).toBe(true);
        expect(isScratchId('_private')).toBe(false);
        expect(isScratchId('client_id')).toBe(false);
        expect(isScratchId(undefined)).toBe(false);
    });
});

describe('sanitizeInfoHtml', () => {
    function toHtml(fragment) {
        const host = document.createElement('div');
        host.appendChild(fragment);
        return host.innerHTML;
    }

    it('keeps allowed tags and attributes', () => {
        const f = sanitizeInfoHtml('<p>Hello <b>world</b> <code>code</code></p>');
        expect(toHtml(f)).toBe('<p>Hello <b>world</b> <code>code</code></p>');
    });

    it('strips script tags (contents become inert text, never executed)', () => {
        const f = sanitizeInfoHtml('<p>a<script>alert(1)</script>b</p>');
        const html = toHtml(f);
        // The <script> tag is gone. Its text content survives as a plain
        // text node, which is safe — it can never execute on re-insertion.
        expect(html).not.toContain('<script');
        expect(html).toContain('>a'); // 'a' before the script
        expect(html).toContain('b</p>');
    });

    it('strips script tags with no text content to nothing visible', () => {
        const f = sanitizeInfoHtml('<p>before<script></script>after</p>');
        const html = toHtml(f);
        expect(html).toBe('<p>beforeafter</p>');
    });

    it('strips event handler attributes', () => {
        const f = sanitizeInfoHtml('<a href="https://example.com" onclick="evil()">link</a>');
        const html = toHtml(f);
        expect(html).toContain('href="https://example.com"');
        expect(html).not.toContain('onclick');
    });

    it('strips javascript: urls', () => {
        const f = sanitizeInfoHtml('<a href="javascript:evil()">bad</a>');
        const html = toHtml(f);
        expect(html).not.toContain('javascript:');
        expect(html).toContain('>bad</a>');
    });

    it('forces rel=noopener on target=_blank links', () => {
        const f = sanitizeInfoHtml('<a href="https://example.com" target="_blank">x</a>');
        const html = toHtml(f);
        expect(html).toContain('target="_blank"');
        expect(html).toContain('rel="noopener noreferrer"');
    });

    it('strips style attributes', () => {
        const f = sanitizeInfoHtml('<p style="color:red">x</p>');
        expect(toHtml(f)).toBe('<p>x</p>');
    });

    it('unwraps disallowed tags to their text content', () => {
        const f = sanitizeInfoHtml('<img src="x"><iframe>iframe content</iframe>');
        // <img> has no text, <iframe> contributes its text
        expect(toHtml(f)).toContain('iframe content');
        expect(toHtml(f)).not.toContain('<img');
        expect(toHtml(f)).not.toContain('<iframe');
    });
});

describe('defaultValues', () => {
    it('resolves defaults per control type', () => {
        const schema = {
            sections: [{
                controls: [
                    { type: 'checkbox', id: 'a', label: 'a', default: true },
                    { type: 'checkbox', id: 'b', label: 'b' /* no default → false */ },
                    { type: 'text',     id: 'c', label: 'c', default: 'hi' },
                    { type: 'number',   id: 'd', label: 'd', default: 5 },
                    { type: 'select',   id: 'e', label: 'e', options: [{ value: '1', label: '1' }] },
                    { type: 'range',    id: 'f', label: 'f', min: 1, max: 10, default: 7 },
                    // info / action are excluded — they carry no persisted value
                    { type: 'info', html: 'ignored' },
                ],
            }],
        };
        expect(defaultValues(schema)).toEqual({
            a: true, b: false, c: 'hi', d: 5, e: '1', f: 7,
        });
    });
});

describe('isVisible', () => {
    it('true when no clause', () => {
        expect(isVisible(undefined, {})).toBe(true);
        expect(isVisible(null, {})).toBe(true);
    });
    it('equals check', () => {
        expect(isVisible({ id: 'mode', equals: 'a' }, { mode: 'a' })).toBe(true);
        expect(isVisible({ id: 'mode', equals: 'a' }, { mode: 'b' })).toBe(false);
    });
    it('oneOf check', () => {
        expect(isVisible({ id: 'mode', oneOf: ['a', 'b'] }, { mode: 'a' })).toBe(true);
        expect(isVisible({ id: 'mode', oneOf: ['a', 'b'] }, { mode: 'c' })).toBe(false);
    });
});

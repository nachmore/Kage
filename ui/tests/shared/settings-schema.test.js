import { describe, it, expect } from 'vitest';
import {
    validateSchema,
    sanitizeInfoHtml,
    defaultValues,
    isVisible,
} from '../../js/shared/settings-schema.js';

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

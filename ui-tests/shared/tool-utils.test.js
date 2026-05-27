import { describe, it, expect } from 'vitest';
import {
    escapeAttr,
    escapeHtml,
    getToolEmoji,
    getToolIcon,
} from '../../ui/js/shared/tool-utils.js';

describe('getToolIcon', () => {
  it('returns search icon for search kinds', () => {
    expect(getToolIcon('search')).toBe('🔍');
    expect(getToolIcon('web_search')).toBe('🔍');
  });

  it('returns edit icon for write kinds', () => {
    expect(getToolIcon('edit')).toBe('✏️');
    expect(getToolIcon('write')).toBe('✏️');
  });

  it('returns read icon', () => {
    expect(getToolIcon('read')).toBe('📖');
  });

  it('returns shell icon', () => {
    expect(getToolIcon('shell')).toBe('💻');
    expect(getToolIcon('terminal')).toBe('💻');
  });

  it('returns default wrench for unknown kinds', () => {
    expect(getToolIcon('unknown')).toBe('🔧');
    expect(getToolIcon('')).toBe('🔧');
    expect(getToolIcon(null)).toBe('🔧');
  });

  it('is case-insensitive', () => {
    expect(getToolIcon('SEARCH')).toBe('🔍');
    expect(getToolIcon('Read')).toBe('📖');
  });
});

describe('getToolEmoji', () => {
  it('returns extension icon for ext: prefix', () => {
    expect(getToolEmoji('ext:my-tool')).toBe('🧩');
  });

  it('matches partial names', () => {
    expect(getToolEmoji('file_search')).toBe('🔍');
    expect(getToolEmoji('read_file')).toBe('📖');
    expect(getToolEmoji('write_to_disk')).toBe('✏️');
    expect(getToolEmoji('run_shell_command')).toBe('💻');
  });

  it('returns cloud icon for AWS tools', () => {
    expect(getToolEmoji('aws_s3_list')).toBe('☁️');
  });

  it('returns default for unknown names', () => {
    expect(getToolEmoji('something')).toBe('🔧');
    expect(getToolEmoji(null)).toBe('🔧');
  });
});

describe('escapeHtml', () => {
    // escapeHtml is for body content (between tags). Browsers render
    // `"` and `'` literally inside body text, so escapeHtml deliberately
    // leaves them alone — escaping them would just emit `&quot;` for
    // every `"` in streamed agent output, padding chunk size for no
    // user-visible benefit. Use escapeAttr when the value lands inside
    // an HTML attribute.

    it('escapes the three body-significant characters', () => {
        expect(escapeHtml('<')).toBe('&lt;');
        expect(escapeHtml('>')).toBe('&gt;');
        expect(escapeHtml('&')).toBe('&amp;');
    });

    it('escapes angle brackets but not quotes', () => {
        // Quotes are intentionally left alone for body content.
        expect(escapeHtml('<script>alert("xss")</script>')).toBe(
            '&lt;script&gt;alert("xss")&lt;/script&gt;'
        );
    });

    it('escapes ampersands once (no double-encoding)', () => {
        expect(escapeHtml('a & b')).toBe('a &amp; b');
        expect(escapeHtml('&amp;')).toBe('&amp;amp;');
    });

    it('null and undefined collapse to empty string', () => {
        expect(escapeHtml(null)).toBe('');
        expect(escapeHtml(undefined)).toBe('');
    });

    it('coerces non-strings via String()', () => {
        expect(escapeHtml(0)).toBe('0');
        expect(escapeHtml(false)).toBe('false');
    });

    it('passes through safe text', () => {
        expect(escapeHtml('hello world')).toBe('hello world');
        expect(escapeHtml('')).toBe('');
    });
});

describe('escapeAttr', () => {
    // escapeAttr is the attribute-safe cousin: same as escapeHtml plus
    // `"` and `'` so the value can never break out of either kind of
    // quote delimiter. The XSS hole this guards against: a string like
    // `" onclick="alert(1)` injected into `value="${escapeHtml(s)}"`
    // would close the value and inject an event handler.

    it('escapes both kinds of quotes in addition to angle brackets', () => {
        expect(escapeAttr('"')).toBe('&quot;');
        expect(escapeAttr("'")).toBe('&#39;');
    });

    it('blocks the canonical attribute-injection payload', () => {
        const payload = '" onclick="alert(1)"';
        const out = escapeAttr(payload);
        // The literal `"` characters that would have closed an enclosing
        // attribute value must be entity-encoded.
        expect(out).not.toContain('"');
        expect(out).toContain('&quot;');
        // Surrounding text is preserved so users still see the raw payload
        // after decode (e.g. when this lands in a `title` attribute).
        expect(out).toContain('onclick=');
    });

    it('escapes all five HTML-significant chars, ampersand once', () => {
        expect(escapeAttr('& < > " \'')).toBe('&amp; &lt; &gt; &quot; &#39;');
    });

    it('null and undefined collapse to empty string', () => {
        expect(escapeAttr(null)).toBe('');
        expect(escapeAttr(undefined)).toBe('');
    });
});

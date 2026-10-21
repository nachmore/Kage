/**
 * Tests for ui/js/shared/internal-hosts.js (which hosts must not be sent to
 * third-party services) and its use in streaming-utils' source chips.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import {
    isInternalHost,
    isNonPublicHost,
    setInternalSiteProfiles,
} from '../../ui/js/shared/internal-hosts.js';
import { addSource, renderSourceChipsHtml } from '../../ui/js/shared/streaming-utils.js';

describe('internal hosts', () => {
    beforeEach(() => setInternalSiteProfiles([]));

    it('flags single-label, IP-literal and reserved-suffix hosts', () => {
        expect(isNonPublicHost('wiki')).toBe(true);
        expect(isNonPublicHost('10.1.2.3')).toBe(true);
        expect(isNonPublicHost('[::1]')).toBe(true);
        expect(isNonPublicHost('printer.local')).toBe(true);
        expect(isNonPublicHost('build.internal')).toBe(true);
        expect(isNonPublicHost('nas.home.arpa')).toBe(true);
        expect(isNonPublicHost('example.com')).toBe(false);
        expect(isNonPublicHost('docs.github.com')).toBe(false);
    });

    it('applies enabled profiles’ URL rules only', () => {
        setInternalSiteProfiles([
            { enabled: true, url_rules: [{ kind: 'domain_suffix', pattern: 'corp.example.com' }] },
            { enabled: true, url_rules: [{ kind: 'regex', pattern: '^https://[^/]*\\.intra\\.example\\.org/' }] },
            { enabled: false, url_rules: [{ kind: 'domain_suffix', pattern: 'off.example.net' }] },
            { enabled: true, url_rules: [{ kind: 'regex', pattern: '(' }] },
        ]);
        expect(isInternalHost('wiki.corp.example.com')).toBe(true);
        expect(isInternalHost('corp.example.com')).toBe(true);
        expect(isInternalHost('notcorp.example.com')).toBe(false);
        expect(isInternalHost('a.intra.example.org')).toBe(true);
        expect(isInternalHost('a.off.example.net')).toBe(false);
    });

    it('keeps internal source chips off the third-party favicon service', () => {
        setInternalSiteProfiles([
            { url_rules: [{ kind: 'domain_suffix', pattern: 'corp.example.com' }] },
        ]);
        const state = {};
        addSource('https://wiki.corp.example.com/page', 'Wiki', null, state);
        addSource('https://docs.github.com/x', 'Docs', null, state);
        const [internal, external] = state.toolSources;
        expect(internal.favicon).toBe('');
        expect(external.favicon).toContain('docs.github.com');
        const html = renderSourceChipsHtml([internal]);
        expect(html).not.toContain('<img');
        expect(html).toContain('wiki.corp.example.com');
    });
});

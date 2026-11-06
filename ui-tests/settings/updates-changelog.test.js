import { describe, it, expect } from 'vitest';
import { tidyReleaseNotes } from '../../ui/js/settings/updates.js';

describe('tidyReleaseNotes', () => {
    it('turns the Full Changelog line into a short link', () => {
        const md = '## What\'s Changed\n* Fix things\n\n**Full Changelog**: https://github.com/nachmore/Kage/compare/dev-1...dev-2';
        expect(tidyReleaseNotes(md)).toContain(
            '[**Full Changelog**](https://github.com/nachmore/Kage/compare/dev-1...dev-2)'
        );
        expect(tidyReleaseNotes(md)).not.toMatch(/\*\*Full Changelog\*\*: https/);
    });

    it('shortens PR and issue references to #number', () => {
        const md = '* Better search by @someone in https://github.com/nachmore/Kage/pull/123\n* See https://github.com/nachmore/Kage/issues/7.';
        const out = tidyReleaseNotes(md);
        expect(out).toContain('in [#123](https://github.com/nachmore/Kage/pull/123)');
        expect(out).toContain('See [#7](https://github.com/nachmore/Kage/issues/7).');
    });

    it('leaves existing markdown links and other text alone', () => {
        const md = '[docs](https://github.com/nachmore/Kage/pull/5) and https://example.com/a/very/long/path';
        expect(tidyReleaseNotes(md)).toBe(md);
    });

    it('handles empty and non-string input', () => {
        expect(tidyReleaseNotes('')).toBe('');
        expect(tidyReleaseNotes(undefined)).toBe('');
    });
});

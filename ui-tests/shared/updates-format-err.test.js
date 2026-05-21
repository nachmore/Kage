/**
 * Tests for the `formatErr` helper in ui/js/settings/updates.js.
 *
 * Background: when an `invoke('check_for_update')` call rejected, the
 * settings UI was rendering "Update check failed: [object Object]"
 * because it called `String(e)` on a Tauri error that arrives as a
 * `{ kind, message }` object (see src/error.rs::AppError). Users
 * couldn't read what actually went wrong.
 *
 * `formatErr` is the small coercion layer that fixed it. These tests
 * lock the contract in so the regression can't sneak back.
 */

import { describe, expect, it } from 'vitest';
import { formatErr } from '../../ui/js/settings/updates.js';

describe('formatErr', () => {
    it('returns a string verbatim', () => {
        expect(formatErr('Boom')).toBe('Boom');
    });

    it('reads .message off an Error instance', () => {
        expect(formatErr(new Error('Network down'))).toBe('Network down');
    });

    it('reads .message off a Tauri-style {kind, message} payload', () => {
        // This is the canonical case — AppError serializes this shape.
        expect(formatErr({ kind: 'internal', message: 'Check failed: 404' })).toBe(
            'Check failed: 404'
        );
    });

    it('handles an Error with empty .message by falling back', () => {
        // A real Error with no message at least gives us the "Error" string,
        // which is more useful than the empty string.
        const e = new Error('');
        expect(formatErr(e)).toBe(String(e));
    });

    it('JSON-stringifies a plain object without a message field', () => {
        expect(formatErr({ kind: 'config', detail: 'no endpoint' })).toBe(
            '{"kind":"config","detail":"no endpoint"}'
        );
    });

    it('falls back to String() on circular objects (JSON would throw)', () => {
        const circ = {};
        circ.self = circ;
        // We don't care about the exact return value here, only that
        // we don't throw and we don't return literal "[object Object]"
        // for the kind/message case the JSON branch catches.
        expect(() => formatErr(circ)).not.toThrow();
    });

    it('returns "Unknown error" for null / undefined', () => {
        expect(formatErr(null)).toBe('Unknown error');
        expect(formatErr(undefined)).toBe('Unknown error');
    });

    it('coerces other primitives via String()', () => {
        expect(formatErr(42)).toBe('42');
        expect(formatErr(false)).toBe('false');
    });
});

/**
 * Tests for ui/js/shared/error-message.js — the helper that bridges
 * Rust's structured `AppError` ({ kind, message }) with JS's varied
 * error shapes (strings, Error instances, raw rejects, etc.).
 *
 * The contract:
 *   - errMessage(e) always returns a useful string
 *   - errKind(e) returns the AppError kind or null
 *   - errLabel(label, e) → `${label}: ${errMessage(e)}`
 *
 * If this drifts, every "Error: ..." UI surface starts showing
 * "[object Object]" again — exactly the regression that motivated
 * the helper.
 */

import { describe, it, expect } from 'vitest';
import { errKind, errLabel, errMessage } from '../../ui/js/shared/error-message.js';

describe('errMessage', () => {
    it('returns strings unchanged', () => {
        expect(errMessage('boom')).toBe('boom');
        expect(errMessage('')).toBe('');
    });

    it('extracts .message from Error instances', () => {
        const e = new Error('something broke');
        expect(errMessage(e)).toBe('something broke');
    });

    it('falls back to String(e) when an Error has no message', () => {
        // Some legacy code throws `new Error()` with empty message; we
        // still need to render *something*, just not the empty string.
        const e = new Error('');
        expect(errMessage(e)).toBe(String(e));
    });

    it('extracts .message from AppError-shaped objects', () => {
        // This is THE case — Rust's AppError serialises to {kind, message}.
        const appError = { kind: 'connection_lost', message: 'Server gone' };
        expect(errMessage(appError)).toBe('Server gone');
    });

    it('handles AppError without ignoring the kind for non-errMessage callers', () => {
        // errMessage only returns the message text; the kind is reachable
        // via errKind. Verify they don't entangle.
        const appError = { kind: 'rate_limited', message: 'Slow down' };
        expect(errMessage(appError)).toBe('Slow down');
        expect(errKind(appError)).toBe('rate_limited');
    });

    it('JSON-encodes objects with no .message', () => {
        // A bare object reject (not AppError-shaped) — preserve enough
        // info that a user can include it in a bug report.
        const o = { code: 42, detail: 'unknown' };
        expect(errMessage(o)).toBe(JSON.stringify(o));
    });

    it('handles null and undefined', () => {
        expect(errMessage(null)).toBe('Unknown error');
        expect(errMessage(undefined)).toBe('Unknown error');
    });

    it('stringifies primitives that fall through', () => {
        // Numbers and booleans land here when callers reject with non-
        // standard values (rare but possible).
        expect(errMessage(42)).toBe('42');
        expect(errMessage(false)).toBe('false');
    });

    it('survives objects with circular refs', () => {
        // JSON.stringify throws on cycles — the helper must fall back
        // to String() rather than re-throwing.
        const o = { name: 'x' };
        o.self = o;
        // No throw; some kind of string back. Concrete value is impl-
        // defined, just verify it doesn't blow up.
        expect(typeof errMessage(o)).toBe('string');
    });
});

describe('errKind', () => {
    it('returns the kind from AppError-shaped objects', () => {
        expect(errKind({ kind: 'connection_lost', message: 'x' })).toBe('connection_lost');
    });

    it('returns null for non-AppError values', () => {
        expect(errKind('boom')).toBeNull();
        expect(errKind(new Error('x'))).toBeNull();
        expect(errKind({ message: 'no kind' })).toBeNull();
        expect(errKind(null)).toBeNull();
        expect(errKind(undefined)).toBeNull();
        expect(errKind(42)).toBeNull();
    });

    it('rejects non-string kind fields', () => {
        // Defensive — a bug somewhere could put a non-string in `kind`.
        // The helper must not return it.
        expect(errKind({ kind: 123, message: 'x' })).toBeNull();
        expect(errKind({ kind: null, message: 'x' })).toBeNull();
    });
});

describe('errLabel', () => {
    it('joins label and extracted message with ": "', () => {
        expect(errLabel('Failed', 'oops')).toBe('Failed: oops');
        expect(errLabel('Failed', new Error('boom'))).toBe('Failed: boom');
        expect(errLabel('Failed', { kind: 'x', message: 'detail' })).toBe('Failed: detail');
    });

    it('handles unknown error gracefully', () => {
        // The "Unknown error" fallback flows through the label so the
        // user sees something meaningful even when we have nothing.
        expect(errLabel('Failed', null)).toBe('Failed: Unknown error');
        expect(errLabel('Failed', undefined)).toBe('Failed: Unknown error');
    });
});

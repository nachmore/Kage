/**
 * Tests for installGlobalErrorCapture — uncaught exceptions and
 * unhandled promise rejections must land in the backend app log.
 *
 * Console interception alone misses these: a ReferenceError thrown from
 * an event handler (e.g. a missing import surfacing at call time) never
 * passes through console.error. The suggestions-dropdown regression was
 * exactly this shape and left no trace in app.jsonl.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { installGlobalErrorCapture } from '../../ui/js/shared/kage-log.js';

function installInvokeSpy() {
    const calls = [];
    window.__TAURI__ = {
        core: {
            invoke: vi.fn(async (cmd, args) => {
                calls.push({ cmd, args });
            }),
        },
    };
    return calls;
}

describe('installGlobalErrorCapture', () => {
    beforeEach(() => {
        delete window.__TAURI__;
    });

    it('forwards uncaught errors to app_log_write with stack and location', () => {
        const calls = installInvokeSpy();
        installGlobalErrorCapture('floating');

        const err = new ReferenceError('measureTextareaContentHeight is not defined');
        window.dispatchEvent(
            new ErrorEvent('error', {
                message: err.message,
                filename: 'http://tauri.localhost/js/floating/app/search.js',
                lineno: 105,
                colno: 9,
                error: err,
            })
        );

        expect(calls.length).toBe(1);
        expect(calls[0].cmd).toBe('app_log_write');
        expect(calls[0].args.level).toBe('error');
        expect(calls[0].args.source).toBe('floating');
        expect(calls[0].args.msg).toContain('measureTextareaContentHeight is not defined');
        expect(calls[0].args.msg).toContain('search.js:105');
    });

    it('forwards unhandled promise rejections', () => {
        const calls = installInvokeSpy();
        installGlobalErrorCapture('floating');

        // jsdom lacks a PromiseRejectionEvent constructor; a plain Event
        // with a `reason` property exercises the same listener.
        const evt = new Event('unhandledrejection');
        evt.reason = new Error('async boom');
        window.dispatchEvent(evt);

        expect(calls.length).toBe(1);
        expect(calls[0].args.level).toBe('error');
        expect(calls[0].args.msg).toContain('unhandledrejection');
        expect(calls[0].args.msg).toContain('async boom');
    });

    it('formats non-Error rejection reasons', () => {
        const calls = installInvokeSpy();
        installGlobalErrorCapture('floating');

        const evt = new Event('unhandledrejection');
        evt.reason = { code: 'connection_lost', message: 'gone' };
        window.dispatchEvent(evt);

        expect(calls.length).toBe(1);
        expect(calls[0].args.msg).toContain('connection_lost');
    });

    it('is idempotent — double install logs each error once', () => {
        const calls = installInvokeSpy();
        installGlobalErrorCapture('floating');
        installGlobalErrorCapture('floating'); // e.g. interceptConsole + direct call

        window.dispatchEvent(
            new ErrorEvent('error', { message: 'once', error: new Error('once') })
        );
        expect(calls.length).toBe(1);
    });
});

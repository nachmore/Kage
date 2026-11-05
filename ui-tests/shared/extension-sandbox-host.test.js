/**
 * Focused tests for ExtensionSandbox's capability enforcement at the
 * invoke bridge. We don't spin up a real iframe here — iframes with
 * null origins and message ports are awkward to test in jsdom. Instead
 * we exercise the handler directly via a minimal fake port, which is
 * exactly where the authoritative permission check lives.
 */

import { describe, it, expect, vi } from 'vitest';
import { ExtensionSandbox } from '../../ui/js/shared/extension-sandbox-host.js';

function makeSandbox({ capabilities, rawInvoke }) {
    const sb = new ExtensionSandbox(
        { extensionId: 'test-ext', capabilities, config: {}, sources: {} },
        rawInvoke,
        document.body,
    );
    // Inject a fake port we control. Messages posted by the host go to
    // `sent`; we synthesise inbound messages via `_onPortMessage`.
    const sent = [];
    sb._port = {
        postMessage: (m) => { sent.push(m); },
        close: () => {},
    };
    sb._ready = true; // pretend init finished
    return { sb, sent };
}

describe('ExtensionSandbox._handleInvoke', () => {
    it('allows a command whose required capability is held', async () => {
        const rawInvoke = vi.fn().mockResolvedValue({ ok: true });
        const { sb, sent } = makeSandbox({
            capabilities: ['clipboard'],
            rawInvoke,
        });

        await sb._handleInvoke({ id: 1, command: 'read_clipboard', args: {} });

        expect(rawInvoke).toHaveBeenCalledWith('read_clipboard', {});
        expect(sent[0]).toEqual({ type: 'invoke-response', id: 1, result: { ok: true } });
    });

    it('rejects a command when the capability is not held', async () => {
        const rawInvoke = vi.fn();
        const { sb, sent } = makeSandbox({
            capabilities: ['storage'], // no 'clipboard'
            rawInvoke,
        });

        await sb._handleInvoke({ id: 2, command: 'read_clipboard', args: {} });

        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent[0].type).toBe('invoke-response');
        expect(sent[0].id).toBe(2);
        expect(sent[0].error).toMatch(/missing capability 'clipboard'/);
    });

    it('rejects commands explicitly marked null, no matter what caps are held', async () => {
        const rawInvoke = vi.fn();
        const { sb, sent } = makeSandbox({
            // Grant everything possible — the forbidden list must still block.
            capabilities: ['storage','clipboard','urls','launch','network','oauth','filesystem','window','windows','notifications','calendar','session','agent','activity','automation','tts'],
            rawInvoke,
        });

        await sb._handleInvoke({ id: 3, command: 'quit_app', args: {} });

        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent[0].error).toMatch(/never callable from an extension/);
    });

    it('rejects unknown commands (fail closed)', async () => {
        const rawInvoke = vi.fn();
        const { sb, sent } = makeSandbox({
            capabilities: ['storage'],
            rawInvoke,
        });

        await sb._handleInvoke({ id: 4, command: 'mystery_command', args: {} });

        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent[0].error).toMatch(/not available to extensions/);
    });

    it('propagates errors thrown by the underlying invoke', async () => {
        const rawInvoke = vi.fn().mockRejectedValue(new Error('backend boom'));
        const { sb, sent } = makeSandbox({
            capabilities: ['urls'],
            rawInvoke,
        });

        await sb._handleInvoke({ id: 5, command: 'open_url', args: { url: 'https://example.com' } });

        expect(rawInvoke).toHaveBeenCalledWith('open_url', { url: 'https://example.com' });
        expect(sent[0].error).toMatch(/backend boom/);
        expect(sent[0]).not.toHaveProperty('errorKind');
    });

    it('forwards an AppError kind and key so the extension can branch on them', async () => {
        const rawInvoke = vi.fn().mockRejectedValue({
            kind: 'timeout',
            key: 'errors.passthrough',
            message: 'OAuth flow timed out before a callback arrived',
        });
        const { sb, sent } = makeSandbox({ capabilities: ['oauth'], rawInvoke });

        await sb._handleInvoke({
            id: 7,
            command: 'oauth_loopback_await',
            args: { args: { listener_id: 'abc' } },
        });

        expect(sent[0]).toEqual({
            type: 'invoke-response',
            id: 7,
            error: 'OAuth flow timed out before a callback arrived',
            errorKind: 'timeout',
            errorKey: 'errors.passthrough',
        });
    });

    it('does not trust args to override the extension identity', async () => {
        // Storage commands are scoped per extension on the backend, so the
        // host force-injects extensionId into args from its own record
        // before forwarding. Any value the sandbox supplied is overwritten.
        // This blocks cross-extension data theft via the storage capability.
        // The arg name MUST be camelCase — Tauri 2's IPC layer renames
        // the Rust param `extension_id: String` to `extensionId` over
        // the wire, and a snake_case field would be silently rejected.
        const rawInvoke = vi.fn().mockResolvedValue(null);
        const { sb } = makeSandbox({
            capabilities: ['storage'],
            rawInvoke,
        });

        await sb._handleInvoke({
            id: 6,
            command: 'save_extension_data',
            args: { key: 'x', data: '{}', extensionId: 'not-me' },
        });

        expect(rawInvoke).toHaveBeenCalled();
        const [, passedArgs] = rawInvoke.mock.calls[0];
        expect(passedArgs.key).toBe('x');
        // The sandbox's claimed identity ('not-me') must NOT survive into
        // the backend call — the host overrides with its own record.
        expect(passedArgs.extensionId).toBe('test-ext');
    });

    it('force-injects extensionId even when storage args omit it', async () => {
        // Even if the sandbox omits extensionId, the host must add it.
        // Otherwise a sandbox could call the bare command and the backend
        // would error out unexpectedly — or worse, fall into a default.
        const rawInvoke = vi.fn().mockResolvedValue(null);
        const { sb } = makeSandbox({
            capabilities: ['storage'],
            rawInvoke,
        });

        await sb._handleInvoke({
            id: 7,
            command: 'load_extension_data',
            args: { key: 'todos' },
        });

        const [, passedArgs] = rawInvoke.mock.calls[0];
        expect(passedArgs.extensionId).toBe('test-ext');
        expect(passedArgs.key).toBe('todos');
    });

    it('does not inject extensionId into non-storage commands', async () => {
        // Only the storage commands need namespacing. Other commands have
        // their own access models and shouldn't get a phantom extensionId
        // arg appearing in their backend call.
        const rawInvoke = vi.fn().mockResolvedValue(null);
        const { sb } = makeSandbox({
            capabilities: ['urls'],
            rawInvoke,
        });

        await sb._handleInvoke({
            id: 8,
            command: 'open_url',
            args: { url: 'https://example.com' },
        });

        const [, passedArgs] = rawInvoke.mock.calls[0];
        expect(passedArgs).toEqual({ url: 'https://example.com' });
        expect(passedArgs.extensionId).toBeUndefined();
    });

    // ---- urls cap: scheme allowlist ----------------------------------

    it('blocks open_url for schemes outside the urls allowlist', async () => {
        const rawInvoke = vi.fn();
        const { sb, sent } = makeSandbox({
            capabilities: ['urls'],
            rawInvoke,
        });

        // file:// can navigate to arbitrary local content; never allowed.
        await sb._handleInvoke({
            id: 100,
            command: 'open_url',
            args: { url: 'file:///etc/passwd' },
        });

        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent[0].error).toMatch(/scheme 'file:'/);
    });

    it('blocks custom app URI schemes from the urls cap', async () => {
        const rawInvoke = vi.fn();
        const { sb, sent } = makeSandbox({
            capabilities: ['urls'],
            rawInvoke,
        });
        // Custom app schemes (spotify://, vscode://, etc.) launch other
        // apps. They need the `launch` cap, not `urls`.
        await sb._handleInvoke({
            id: 101,
            command: 'open_url',
            args: { url: 'spotify://album/foo' },
        });

        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent[0].error).toMatch(/'launch' capability/);
    });

    it('allows http/https with the urls cap', async () => {
        const rawInvoke = vi.fn().mockResolvedValue(null);
        const { sb } = makeSandbox({
            capabilities: ['urls'],
            rawInvoke,
        });

        await sb._handleInvoke({
            id: 102,
            command: 'open_url',
            args: { url: 'http://example.com' },
        });
        await sb._handleInvoke({
            id: 103,
            command: 'open_url',
            args: { url: 'https://example.com/path?q=1' },
        });

        expect(rawInvoke).toHaveBeenCalledTimes(2);
    });

    it('allows mailto/tel and OS-settings deep links with the urls cap', async () => {
        const rawInvoke = vi.fn().mockResolvedValue(null);
        const { sb } = makeSandbox({
            capabilities: ['urls'],
            rawInvoke,
        });

        const allowed = [
            'mailto:nobody@example.com',
            'tel:+15551234567',
            'sms:+15551234567',
            'x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Calendars',
            'ms-settings:privacy-location',
        ];
        for (let i = 0; i < allowed.length; i++) {
            await sb._handleInvoke({
                id: 200 + i,
                command: 'open_url',
                args: { url: allowed[i] },
            });
        }
        expect(rawInvoke).toHaveBeenCalledTimes(allowed.length);
    });

    it('rewrites bare www.* as https for validation', async () => {
        const rawInvoke = vi.fn().mockResolvedValue(null);
        const { sb } = makeSandbox({
            capabilities: ['urls'],
            rawInvoke,
        });
        await sb._handleInvoke({
            id: 300,
            command: 'open_url',
            args: { url: 'www.example.com' },
        });
        expect(rawInvoke).toHaveBeenCalled();
    });

    it('rejects open_url with no url argument', async () => {
        const rawInvoke = vi.fn();
        const { sb, sent } = makeSandbox({
            capabilities: ['urls'],
            rawInvoke,
        });

        await sb._handleInvoke({ id: 400, command: 'open_url', args: {} });

        expect(rawInvoke).not.toHaveBeenCalled();
        expect(sent[0].error).toMatch(/no 'url' argument/);
    });
});

describe('ExtensionSandbox OAuth listener cleanup', () => {
    const cancels = (rawInvoke) =>
        rawInvoke.mock.calls
            .filter(([cmd]) => cmd === 'oauth_loopback_cancel')
            .map(([, args]) => args.args.listener_id);

    it('cancels the listeners the extension left open when it is destroyed', async () => {
        let nextId = 0;
        const rawInvoke = vi.fn(async (cmd) => {
            if (cmd === 'oauth_loopback_start') {
                nextId += 1;
                return { listener_id: `L${nextId}`, redirect_uri: 'http://127.0.0.1:1/cb', port: 1 };
            }
            if (cmd === 'oauth_loopback_await') return { params: { code: 'c' } };
            return null;
        });
        const { sb } = makeSandbox({ capabilities: ['oauth'], rawInvoke });
        const start = (id) =>
            sb._handleInvoke({ id, command: 'oauth_loopback_start', args: { args: { redirect_path: '/cb' } } });

        await start(1); // L1: still waiting for the browser
        await start(2); // L2: awaited to completion
        await start(3); // L3: cancelled by the extension
        await sb._handleInvoke({ id: 4, command: 'oauth_loopback_await', args: { args: { listener_id: 'L2' } } });
        await sb._handleInvoke({ id: 5, command: 'oauth_loopback_cancel', args: { args: { listener_id: 'L3' } } });
        expect(cancels(rawInvoke)).toEqual(['L3']);

        sb.destroy();
        expect(cancels(rawInvoke)).toEqual(['L3', 'L1']);
    });

    it('keeps tracking a listener whose second await was rejected', async () => {
        const rawInvoke = vi.fn(async (cmd) => {
            if (cmd === 'oauth_loopback_start') return { listener_id: 'L1', redirect_uri: 'x', port: 1 };
            if (cmd === 'oauth_loopback_await') {
                throw { kind: 'internal', key: 'errors.oauth.already_awaited', message: 'busy' };
            }
            return null;
        });
        const { sb } = makeSandbox({ capabilities: ['oauth'], rawInvoke });
        await sb._handleInvoke({ id: 1, command: 'oauth_loopback_start', args: { args: { redirect_path: '/cb' } } });
        await sb._handleInvoke({ id: 2, command: 'oauth_loopback_await', args: { args: { listener_id: 'L1' } } });
        sb.destroy();
        expect(cancels(rawInvoke)).toEqual(['L1']);
    });

    it('drops the response of an await that settles after destroy, and cancels a late start', async () => {
        let settleAwait;
        let settleStart;
        const rawInvoke = vi.fn((cmd) => {
            if (cmd === 'oauth_loopback_await') return new Promise((r) => { settleAwait = r; });
            if (cmd === 'oauth_loopback_start') return new Promise((r) => { settleStart = r; });
            return Promise.resolve(null);
        });
        const { sb, sent } = makeSandbox({ capabilities: ['oauth'], rawInvoke });
        const awaiting = sb._handleInvoke({ id: 1, command: 'oauth_loopback_await', args: { args: { listener_id: 'L0' } } });
        const starting = sb._handleInvoke({ id: 2, command: 'oauth_loopback_start', args: { args: { redirect_path: '/cb' } } });

        sb.destroy();
        settleAwait({ params: {} });
        settleStart({ listener_id: 'L9', redirect_uri: 'x', port: 1 });
        await expect(awaiting).resolves.toBeUndefined();
        await expect(starting).resolves.toBeUndefined();

        // Only destroy()'s own cleanup RPC went out; no invoke-response.
        expect(sent.filter((m) => m.type === 'invoke-response')).toEqual([]);
        // The extension never learned L9's id, so the host stops it.
        expect(cancels(rawInvoke)).toEqual(['L9']);
    });
});

describe('ExtensionSandbox invoke rate limiting', () => {
    // read_clipboard needs only the 'clipboard' capability and has no arg
    // validation, so it's the cleanest command to fire in a loop.
    async function fireN(sb, n) {
        for (let i = 0; i < n; i++) {
            await sb._handleInvoke({ id: i, command: 'read_clipboard', args: {} });
        }
    }

    it('allows a normal burst under the cap', async () => {
        const rawInvoke = vi.fn().mockResolvedValue('clip');
        const { sb } = makeSandbox({ capabilities: ['clipboard'], rawInvoke });
        await fireN(sb, 50);
        expect(rawInvoke).toHaveBeenCalledTimes(50);
    });

    it('rejects invokes beyond the per-window cap and stops dispatching them', async () => {
        const rawInvoke = vi.fn().mockResolvedValue('clip');
        const { sb, sent } = makeSandbox({ capabilities: ['clipboard'], rawInvoke });
        await fireN(sb, 250);
        // Only the first 100 reach the host; the rest are rejected.
        expect(rawInvoke).toHaveBeenCalledTimes(100);
        const rejected = sent.filter((m) => /rate limit exceeded/.test(m.error || ''));
        expect(rejected.length).toBe(150);
    });

    it('recovers once the sliding window drains', async () => {
        vi.useFakeTimers();
        try {
            const rawInvoke = vi.fn().mockResolvedValue('clip');
            const { sb } = makeSandbox({ capabilities: ['clipboard'], rawInvoke });
            await fireN(sb, 150); // 100 allowed, 50 rejected
            expect(rawInvoke).toHaveBeenCalledTimes(100);
            // Advance past the window so old timestamps age out.
            vi.advanceTimersByTime(1_100);
            await fireN(sb, 100);
            expect(rawInvoke).toHaveBeenCalledTimes(200); // full budget again
        } finally {
            vi.useRealTimers();
        }
    });

    it('does not count capability-denied calls against the budget', async () => {
        const rawInvoke = vi.fn().mockResolvedValue('clip');
        // No 'clipboard' cap → every read_clipboard is denied at the gate.
        const { sb } = makeSandbox({ capabilities: [], rawInvoke });
        await fireN(sb, 300);
        expect(rawInvoke).not.toHaveBeenCalled();
        // Now grant nothing changed, but a different *allowed* command must
        // still have its full budget — denied calls never consumed it.
        const ok = makeSandbox({ capabilities: ['clipboard'], rawInvoke });
        await fireN(ok.sb, 100);
        expect(rawInvoke).toHaveBeenCalledTimes(100);
    });
});

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
            capabilities: ['storage','clipboard','shell','filesystem','window','windows','notifications','calendar','session','agent','activity','automation','tts'],
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
            capabilities: ['shell'],
            rawInvoke,
        });

        await sb._handleInvoke({ id: 5, command: 'open_url', args: { url: 'x' } });

        expect(rawInvoke).toHaveBeenCalledWith('open_url', { url: 'x' });
        expect(sent[0].error).toMatch(/backend boom/);
    });

    it('does not trust args to override the extension identity', async () => {
        // Storage commands are scoped per extension on the backend, so the
        // host force-injects extension_id into args from its own record
        // before forwarding. Any value the sandbox supplied is overwritten.
        // This blocks cross-extension data theft via the storage capability.
        const rawInvoke = vi.fn().mockResolvedValue(null);
        const { sb } = makeSandbox({
            capabilities: ['storage'],
            rawInvoke,
        });

        await sb._handleInvoke({
            id: 6,
            command: 'save_extension_data',
            args: { key: 'x', data: '{}', extension_id: 'not-me' },
        });

        expect(rawInvoke).toHaveBeenCalled();
        const [, passedArgs] = rawInvoke.mock.calls[0];
        expect(passedArgs.key).toBe('x');
        // The sandbox's claimed identity ('not-me') must NOT survive into
        // the backend call — the host overrides with its own record.
        expect(passedArgs.extension_id).toBe('test-ext');
    });

    it('force-injects extension_id even when storage args omit it', async () => {
        // Even if the sandbox omits extension_id, the host must add it.
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
        expect(passedArgs.extension_id).toBe('test-ext');
        expect(passedArgs.key).toBe('todos');
    });

    it('does not inject extension_id into non-storage commands', async () => {
        // Only the storage commands need namespacing. Other commands have
        // their own access models and shouldn't get a phantom extension_id
        // arg appearing in their backend call.
        const rawInvoke = vi.fn().mockResolvedValue(null);
        const { sb } = makeSandbox({
            capabilities: ['shell'],
            rawInvoke,
        });

        await sb._handleInvoke({
            id: 8,
            command: 'open_url',
            args: { url: 'https://example.com' },
        });

        const [, passedArgs] = rawInvoke.mock.calls[0];
        expect(passedArgs).toEqual({ url: 'https://example.com' });
        expect(passedArgs.extension_id).toBeUndefined();
    });
});

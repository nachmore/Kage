/**
 * End-to-end schema rendering test. Uses a stub sandbox that records
 * calls and replays canned responses — we're verifying the renderer
 * wires inputs correctly and routes actions through the sandbox, not
 * the real iframe transport.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderSchema } from '../../ui/js/shared/settings-renderer.js';

function stubSandbox(responses = {}) {
    const calls = [];
    return {
        calls,
        async call(method, params, opts) {
            calls.push({ method, params, opts });
            if (method in responses) {
                const r = responses[method];
                return typeof r === 'function' ? r(params) : r;
            }
            return undefined;
        },
    };
}

describe('renderSchema', () => {
    let container;

    beforeEach(() => {
        container = document.createElement('div');
        document.body.appendChild(container);
    });

    it('renders primitive controls with defaults and collects values on save', () => {
        const sandbox = stubSandbox();
        const inst = renderSchema({
            extensionId: 'test',
            container,
            sandbox,
            schema: {
                sections: [{
                    controls: [
                        { type: 'checkbox', id: 'a', label: 'A', default: true },
                        { type: 'text',     id: 'b', label: 'B', default: 'hi' },
                        { type: 'number',   id: 'c', label: 'C', default: 4 },
                        { type: 'select',   id: 'd', label: 'D', default: 'x',
                          options: [{ value: 'x', label: 'X' }, { value: 'y', label: 'Y' }] },
                    ],
                }],
            },
        });

        inst.load({ a: false, b: 'loaded', c: 7, d: 'y' });
        expect(inst.save()).toEqual({ a: false, b: 'loaded', c: 7, d: 'y' });
    });

    it('applies showWhen visibility based on a toggle', () => {
        const sandbox = stubSandbox();
        const inst = renderSchema({
            extensionId: 'sw',
            container,
            sandbox,
            schema: {
                sections: [{
                    controls: [
                        { type: 'checkbox', id: 'flag', label: 'Flag', default: false },
                        { type: 'text',     id: 'extra', label: 'Extra',
                          showWhen: { id: 'flag', equals: true } },
                    ],
                }],
            },
        });

        inst.load({ flag: false, extra: '' });
        const row = container.querySelector('#ext-row-sw-extra');
        expect(row.style.display).toBe('none');

        // Toggle the flag — the extra row should become visible.
        const flagInput = container.querySelector('#ext-ctrl-sw-flag');
        flagInput.checked = true;
        flagInput.dispatchEvent(new Event('change'));
        expect(row.style.display).toBe('');
    });

    it('routes action button clicks to the sandbox', async () => {
        const sandbox = stubSandbox({
            runSettingsAction: ({ action, values }) => {
                return { status: `ran ${action} with ${values.mode}` };
            },
        });
        const inst = renderSchema({
            extensionId: 'act',
            container,
            sandbox,
            schema: {
                sections: [{
                    controls: [
                        { type: 'select', id: 'mode', label: 'M', default: 'a',
                          options: [{ value: 'a', label: 'A' }, { value: 'b', label: 'B' }] },
                        { type: 'action', id: 'go', label: 'Go', action: 'do_thing' },
                    ],
                }],
            },
        });

        inst.load({ mode: 'b' });
        const btn = container.querySelector('#ext-ctrl-act-go');
        btn.click();

        // Wait a tick for the click handler to complete.
        await new Promise(r => setTimeout(r, 0));

        expect(sandbox.calls.some(c => c.method === 'runSettingsAction' && c.params.action === 'do_thing'))
            .toBe(true);
    });

    it('confirm=... shows a native confirm and cancels if user says no', async () => {
        const sandbox = stubSandbox({
            runSettingsAction: () => ({ status: 'ran' }),
        });
        const originalConfirm = window.confirm;
        window.confirm = vi.fn().mockReturnValue(false);
        try {
            const inst = renderSchema({
                extensionId: 'cnf',
                container,
                sandbox,
                schema: {
                    sections: [{
                        controls: [
                            { type: 'action', id: 'zap', label: 'Zap',
                              action: 'zap', confirm: 'Really?' },
                        ],
                    }],
                },
            });
            inst.load({});
            container.querySelector('#ext-ctrl-cnf-zap').click();
            await new Promise(r => setTimeout(r, 0));
            expect(window.confirm).toHaveBeenCalledWith('Really?');
            expect(sandbox.calls.some(c => c.method === 'runSettingsAction')).toBe(false);
        } finally {
            window.confirm = originalConfirm;
        }
    });

    it('validate() calls normalize then validate on the sandbox', async () => {
        const sandbox = stubSandbox({
            normalizeSettings: ({ values }) => ({ values: { ...values, b: (values.b || '').trim() } }),
            validateSettings: ({ values }) => {
                if (!values.b) return { valid: false, error: 'b required' };
                return { valid: true };
            },
        });
        const inst = renderSchema({
            extensionId: 'val',
            container,
            sandbox,
            schema: {
                sections: [{
                    controls: [{ type: 'text', id: 'b', label: 'B', default: '' }],
                }],
            },
        });

        inst.load({ b: '  hello  ' });
        const r = await inst.validate();
        expect(r).toEqual({ valid: true });

        // The normalize() result should have been written back to the input.
        const input = container.querySelector('#ext-ctrl-val-b');
        expect(input.value).toBe('hello');
    });

    it('enforces schema-level min/max when extension returns valid', async () => {
        const sandbox = stubSandbox({
            validateSettings: () => ({ valid: true }),
        });
        const inst = renderSchema({
            extensionId: 'mm',
            container,
            sandbox,
            schema: {
                sections: [{
                    controls: [{ type: 'number', id: 'n', label: 'N', default: 5, min: 1, max: 10 }],
                }],
            },
        });

        inst.load({ n: 42 });
        const r = await inst.validate();
        expect(r.valid).toBe(false);
        expect(r.error).toMatch(/at most 10/);
    });

    it('renders an action button as disabled when disabled:true', () => {
        const sandbox = stubSandbox();
        const inst = renderSchema({
            extensionId: 'dis',
            container,
            sandbox,
            schema: {
                sections: [{
                    controls: [
                        { type: 'action', id: 'out', label: 'Sign out', action: 'signout', disabled: true },
                    ],
                }],
            },
        });
        inst.load({});
        const btn = container.querySelector('#ext-ctrl-dis-out');
        expect(btn.disabled).toBe(true);
    });

    it('runs settings actions with a generous (but bounded) RPC timeout', async () => {
        // User-gated flows (OAuth consent) exceed the default 10s cap, so the
        // action RPC gets a much larger timeout — but still finite, so a
        // runaway action can't wedge the button disabled forever.
        const sandbox = stubSandbox({ runSettingsAction: () => ({ status: 'ok' }) });
        const inst = renderSchema({
            extensionId: 'to',
            container,
            sandbox,
            schema: {
                sections: [{ controls: [{ type: 'action', id: 'go', label: 'Go', action: 'do' }] }],
            },
        });
        inst.load({});
        container.querySelector('#ext-ctrl-to-go').click();
        await new Promise((r) => setTimeout(r, 0));

        const call = sandbox.calls.find((c) => c.method === 'runSettingsAction');
        // Well past the 300s OAuth loopback, but bounded (not 0/Infinity).
        expect(call.opts.timeoutMs).toBeGreaterThan(300_000);
        expect(Number.isFinite(call.opts.timeoutMs)).toBe(true);
    });

    it('refresh host effect re-fetches getSettings and re-renders in place', async () => {
        // First getSettings (implicit via renderSchema caller) is the initial
        // schema; the refresh re-fetch returns an updated one where the
        // action label and disabled state have flipped — simulating a
        // connection that just went from signed-in to signed-out.
        let signedIn = true;
        const sandbox = stubSandbox({
            getSettings: () => ({
                sections: [{
                    controls: [
                        { type: 'info', id: 'status', html: signedIn ? 'Signed in.' : 'Not signed in.' },
                        { type: 'action', id: 'connect', label: signedIn ? 'Reconnect' : 'Connect', action: 'connect' },
                        { type: 'action', id: 'signout', label: 'Sign out', action: 'signout', disabled: !signedIn },
                        { type: 'action', id: 'check', label: 'Check', action: 'check' },
                    ],
                }],
            }),
            runSettingsAction: ({ action }) => {
                if (action === 'check') {
                    signedIn = false; // the check discovered we're disconnected
                    return { status: '❌ revoked', host: { type: 'refresh' } };
                }
                return {};
            },
        });

        // Seed the initial render with the signed-in schema.
        const inst = renderSchema({
            extensionId: 'sp',
            container,
            sandbox,
            schema: {
                sections: [{
                    controls: [
                        { type: 'info', id: 'status', html: 'Signed in.' },
                        { type: 'action', id: 'connect', label: 'Reconnect', action: 'connect' },
                        { type: 'action', id: 'signout', label: 'Sign out', action: 'signout', disabled: false },
                        { type: 'action', id: 'check', label: 'Check', action: 'check' },
                    ],
                }],
            },
        });
        inst.load({});

        // Precondition: signed-in labels/state.
        expect(container.querySelector('#ext-ctrl-sp-connect').textContent.trim()).toBe('Reconnect');
        expect(container.querySelector('#ext-ctrl-sp-signout').disabled).toBe(false);

        // Click "Check" → action flips state and requests a refresh.
        container.querySelector('#ext-ctrl-sp-check').click();
        await new Promise((r) => setTimeout(r, 0));

        // The panel re-rendered from the fresh schema.
        expect(sandbox.calls.some((c) => c.method === 'getSettings')).toBe(true);
        expect(container.querySelector('#ext-ctrl-sp-connect').textContent.trim()).toBe('Connect');
        expect(container.querySelector('#ext-ctrl-sp-signout').disabled).toBe(true);
        expect(container.querySelector('#ext-row-sp-status__info').textContent).toMatch(/Not signed in/);
        // The action's status message survived the re-render.
        expect(container.querySelector('#ext-ctrl-sp-check__status').textContent).toBe('❌ revoked');
    });
});

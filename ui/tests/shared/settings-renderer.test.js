/**
 * End-to-end schema rendering test. Uses a stub sandbox that records
 * calls and replays canned responses — we're verifying the renderer
 * wires inputs correctly and routes actions through the sandbox, not
 * the real iframe transport.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderSchema } from '../../js/shared/settings-renderer.js';

function stubSandbox(responses = {}) {
    const calls = [];
    return {
        calls,
        async call(method, params) {
            calls.push({ method, params });
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
});

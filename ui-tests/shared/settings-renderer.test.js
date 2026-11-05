/**
 * End-to-end schema rendering test. Uses a stub sandbox that records
 * calls and replays canned responses — we're verifying the renderer
 * wires inputs correctly and routes actions through the sandbox, not
 * the real iframe transport.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
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

    it('info-block links are neutralized: no live href for the Settings webview to follow', () => {
        // Spotify's setup text. Rendered with a live href and no target, a
        // click navigated the whole Settings window to Spotify's dashboard.
        const inst = renderSchema({
            extensionId: 'spotify',
            container,
            sandbox: stubSandbox(),
            schema: {
                sections: [{
                    controls: [{
                        type: 'info',
                        html:
                            'Create one free at <a href="https://developer.spotify.com/dashboard">developer.spotify.com/dashboard</a>' +
                            ' or <a href="https://example.com/x" target="_blank">here</a>.' +
                            ' <a href="javascript:alert(1)">bad</a>',
                    }],
                }],
            },
        });
        inst.load({});

        const [dash, blank, bad] = container.querySelectorAll('.setting-info-body a');
        expect(dash.getAttribute('href')).toBe('#');
        expect(dash.getAttribute('data-href')).toBe('https://developer.spotify.com/dashboard');
        expect(blank.getAttribute('href')).toBe('#');
        expect(blank.getAttribute('data-href')).toBe('https://example.com/x');
        expect(blank.hasAttribute('target')).toBe(false);
        // The info sanitizer already dropped the javascript: href.
        expect(bad.hasAttribute('href')).toBe(false);
        expect(bad.hasAttribute('data-href')).toBe(false);
    });

    it('a click on an info-block link opens it once via open_url (with the window link handler)', async () => {
        vi.resetModules();
        const { renderSchema: render } = await import('../../ui/js/shared/settings-renderer.js');
        const lh = await import('../../ui/js/shared/link-handler.js');
        const invoke = vi.fn(() => Promise.resolve());
        lh.initLinkHandler(invoke);
        try {
            const inst = render({
                // Distinct id: earlier tests' containers stay in the document,
                // and jsdom's #id lookup returns the first match document-wide.
                extensionId: 'spotify-click',
                container,
                sandbox: stubSandbox(),
                schema: {
                    sections: [{
                        controls: [{
                            type: 'info',
                            html: '<a href="https://developer.spotify.com/dashboard">dashboard</a>',
                        }],
                    }],
                },
            });
            inst.load({});
            const a = container.querySelector('.setting-info-body a');
            const ev = new MouseEvent('click', { bubbles: true, cancelable: true });
            a.dispatchEvent(ev);

            expect(ev.defaultPrevented).toBe(true);
            expect(invoke).toHaveBeenCalledTimes(1);
            expect(invoke).toHaveBeenCalledWith('open_url', {
                url: 'https://developer.spotify.com/dashboard',
            });
        } finally {
            lh.disposeLinkHandler();
        }
    });

    it('shows the localised "working" status while an action runs', async () => {
        // i18n isn't initialised in tests, so t() returns the key itself.
        let finish;
        const sandbox = stubSandbox({
            runSettingsAction: () => new Promise((r) => { finish = r; }),
        });
        const inst = renderSchema({
            extensionId: 'wrk',
            container,
            sandbox,
            schema: { sections: [{ controls: [{ type: 'action', id: 'go', label: 'Go', action: 'go' }] }] },
        });
        inst.load({});
        container.querySelector('#ext-ctrl-wrk-go').click();
        const status = container.querySelector('#ext-ctrl-wrk-go__status');
        expect(status.textContent).toBe('settings.ext_settings.action.working');
        expect(status.getAttribute('aria-live')).toBe('polite');
        finish({ status: 'done' });
        await new Promise((r) => setTimeout(r, 0));
        expect(status.textContent).toBe('done');
    });

    it('refresh keeps the status on the action control even when its id differs from its RPC name', async () => {
        const schema = {
            sections: [{ controls: [{ type: 'action', id: 'check_btn', label: 'Check', action: 'check_connection' }] }],
        };
        const sandbox = stubSandbox({
            getSettings: () => schema,
            runSettingsAction: () => ({ status: 'still connected', host: { type: 'refresh' } }),
        });
        const inst = renderSchema({ extensionId: 'rid', container, sandbox, schema });
        inst.load({});
        container.querySelector('#ext-ctrl-rid-check_btn').click();
        await new Promise((r) => setTimeout(r, 0));
        expect(container.querySelector('#ext-ctrl-rid-check_btn__status').textContent).toBe('still connected');
    });
});

describe('copy actions', () => {
    let container;
    let writeText;

    beforeEach(() => {
        container = document.createElement('div');
        document.body.appendChild(container);
        writeText = vi.fn(() => Promise.resolve());
        Object.defineProperty(navigator, 'clipboard', {
            value: { writeText },
            configurable: true,
        });
    });

    afterEach(() => {
        delete navigator.clipboard;
        vi.useRealTimers();
    });

    function render(extensionId, controls, sandbox = stubSandbox()) {
        const inst = renderSchema({ extensionId, container, sandbox, schema: { sections: [{ controls }] } });
        inst.load({});
        return { inst, sandbox };
    }

    const flush = () => new Promise((r) => setTimeout(r, 0));

    it('writes a literal value synchronously inside the click, with no sandbox round-trip', async () => {
        const { sandbox } = render('cp1', [
            { type: 'action', id: 'copy_uri', label: 'Copy', action: 'legacy_copy',
              copy: { value: 'http://127.0.0.1:43821/spotify/callback' } },
        ]);
        container.querySelector('#ext-ctrl-cp1-copy_uri').click();
        // Already called by the time click() returns: no await happened first,
        // so the click's user activation is still live.
        expect(writeText).toHaveBeenCalledWith('http://127.0.0.1:43821/spotify/callback');

        await flush();
        expect(sandbox.calls.some((c) => c.method === 'runSettingsAction')).toBe(false);
        expect(container.querySelector('#ext-ctrl-cp1-copy_uri__status').textContent)
            .toBe('settings.ext_settings.action.copied');
    });

    it('copies the live value of the referenced control', async () => {
        render('cp2', [
            { type: 'text', id: '__client_id', label: 'Client ID', default: '' },
            { type: 'action', id: 'copy_id', label: 'Copy', copy: { from: '__client_id' } },
        ]);
        const input = container.querySelector('#ext-ctrl-cp2-__client_id');
        input.value = 'abc123';
        input.dispatchEvent(new Event('input'));
        container.querySelector('#ext-ctrl-cp2-copy_id').click();
        expect(writeText).toHaveBeenCalledWith('abc123');
    });

    it('says there is nothing to copy when the source control is empty', async () => {
        render('cp3', [
            { type: 'text', id: 'src', label: 'Src', default: '' },
            { type: 'action', id: 'copy_src', label: 'Copy', copy: { from: 'src' } },
        ]);
        container.querySelector('#ext-ctrl-cp3-copy_src').click();
        await flush();
        expect(writeText).not.toHaveBeenCalled();
        expect(container.querySelector('#ext-ctrl-cp3-copy_src__status').textContent)
            .toBe('settings.ext_settings.action.copy_empty');
    });

    it('says there is nothing to copy for an empty number field, not its default', async () => {
        render('cp3n', [
            { type: 'number', id: 'port', label: 'Port', default: 8080 },
            { type: 'action', id: 'copy_port', label: 'Copy', copy: { from: 'port' } },
        ]);
        const input = container.querySelector('#ext-ctrl-cp3n-port');
        expect(input.value).toBe('8080');
        container.querySelector('#ext-ctrl-cp3n-copy_port').click();
        expect(writeText).toHaveBeenLastCalledWith('8080');
        await flush();

        input.value = '';
        container.querySelector('#ext-ctrl-cp3n-copy_port').click();
        await flush();
        expect(writeText).toHaveBeenCalledTimes(1);
        expect(container.querySelector('#ext-ctrl-cp3n-copy_port__status').textContent)
            .toBe('settings.ext_settings.action.copy_empty');
    });

    it('refuses a typed value that is too long for the clipboard', async () => {
        const { inst } = render('cp4', [
            { type: 'text', id: 'src', label: 'Src', default: '' },
            { type: 'action', id: 'copy_src', label: 'Copy', copy: { from: 'src' } },
        ]);
        inst.log = { warn: vi.fn() };
        container.querySelector('#ext-ctrl-cp4-src').value = 'x'.repeat(5000);
        container.querySelector('#ext-ctrl-cp4-copy_src').click();
        await flush();
        expect(writeText).not.toHaveBeenCalled();
        expect(container.querySelector('#ext-ctrl-cp4-copy_src__status').textContent)
            .toBe('settings.ext_settings.action.copy_failed');
    });

    it('reports a failed clipboard write', async () => {
        writeText.mockImplementation(() => Promise.reject(new Error('denied')));
        const { inst } = render('cp5', [
            { type: 'action', id: 'cp', label: 'Copy', copy: { value: 'x' } },
        ]);
        inst.log = { warn: vi.fn() };
        container.querySelector('#ext-ctrl-cp5-cp').click();
        await flush();
        expect(container.querySelector('#ext-ctrl-cp5-cp__status').textContent)
            .toBe('settings.ext_settings.action.copy_failed');
    });

    it('reports failure when there is no clipboard API at all', async () => {
        delete navigator.clipboard;
        const { inst } = render('cp6', [
            { type: 'action', id: 'cp', label: 'Copy', copy: { value: 'x' } },
        ]);
        inst.log = { warn: vi.fn() };
        container.querySelector('#ext-ctrl-cp6-cp').click();
        await flush();
        expect(container.querySelector('#ext-ctrl-cp6-cp__status').textContent)
            .toBe('settings.ext_settings.action.copy_failed');
    });

    it('clears the "Copied" status after a moment', async () => {
        vi.useFakeTimers();
        render('cp7', [{ type: 'action', id: 'cp', label: 'Copy', copy: { value: 'x' } }]);
        container.querySelector('#ext-ctrl-cp7-cp').click();
        await vi.advanceTimersByTimeAsync(0);
        const status = container.querySelector('#ext-ctrl-cp7-cp__status');
        expect(status.textContent).toBe('settings.ext_settings.action.copied');
        await vi.advanceTimersByTimeAsync(2500);
        expect(status.textContent).toBe('');
    });
});

describe('persisting values', () => {
    let container;

    beforeEach(() => {
        container = document.createElement('div');
        document.body.appendChild(container);
    });

    const controls = [
        { type: 'text', id: 'trigger', label: 'Trigger', default: 'sp' },
        { type: 'checkbox', id: 'show_bar', label: 'Show bar', default: true },
        { type: 'text', id: '__setup_client_id', label: 'Client ID', default: 'from-extension' },
    ];

    function render(extensionId, sandbox = stubSandbox()) {
        return renderSchema({ extensionId, container, sandbox, schema: { sections: [{ controls }] } });
    }

    it('never persists scratch (__) fields, but actions still receive them', async () => {
        const sandbox = stubSandbox({ runSettingsAction: () => ({}) });
        const inst = render('pv1', sandbox);
        inst.load({ trigger: 'sp', show_bar: true });
        container.querySelector('#ext-ctrl-pv1-__setup_client_id').value = 'typed';

        expect(inst.save()).toEqual({ trigger: 'sp', show_bar: true });
        expect(inst.currentValues()).toMatchObject({ __setup_client_id: 'typed' });

        const btnSchema = { type: 'action', id: 'save_id', label: 'Save', action: 'save_id' };
        const inst2 = renderSchema({
            extensionId: 'pv1b',
            container,
            sandbox,
            schema: { sections: [{ controls: [...controls, btnSchema] }] },
        });
        inst2.load({});
        container.querySelector('#ext-ctrl-pv1b-__setup_client_id').value = 'typed';
        container.querySelector('#ext-ctrl-pv1b-save_id').click();
        await new Promise((r) => setTimeout(r, 0));
        const call = sandbox.calls.find((c) => c.method === 'runSettingsAction');
        expect(call.params.values.__setup_client_id).toBe('typed');
    });

    it('ignores scratch values an older host already wrote to config', () => {
        const inst = render('pv2');
        inst.load({ trigger: 'x', show_bar: false, __setup_client_id: 'stale' });
        expect(container.querySelector('#ext-ctrl-pv2-__setup_client_id').value).toBe('from-extension');
        expect(inst.save()).toEqual({ trigger: 'x', show_bar: false });
    });

    it('saves what normalize() rewrote and keeps the keys it left out', async () => {
        const sandbox = stubSandbox({
            // Returns only the key it changed (older extensions do this),
            // plus one that isn't a control.
            normalizeSettings: ({ values }) => ({
                values: { trigger: values.trigger.trim().toLowerCase(), injected: 'nope' },
            }),
        });
        const inst = render('pv3', sandbox);
        inst.load({ trigger: '  SP ', show_bar: false });

        expect((await inst.validate()).valid).toBe(true);
        expect(container.querySelector('#ext-ctrl-pv3-trigger').value).toBe('sp');
        // `show_bar` is kept at its live value, not erased from config;
        // `injected` isn't a control; scratch is never saved.
        expect(inst.save()).toEqual({ trigger: 'sp', show_bar: false });

        // An edit after validate() is saved as-is.
        const cb = container.querySelector('#ext-ctrl-pv3-show_bar');
        cb.checked = true;
        cb.dispatchEvent(new Event('change'));
        expect(inst.save()).toEqual({ trigger: 'sp', show_bar: true });
    });

    it('a refresh carries in-progress scratch values across the re-render', async () => {
        const schema = { sections: [{ controls: [
            ...controls,
            { type: 'action', id: 'next', label: 'Next', action: 'next' },
        ] }] };
        const sandbox = stubSandbox({
            getSettings: () => schema,
            runSettingsAction: () => ({ host: { type: 'refresh' } }),
        });
        const inst = renderSchema({ extensionId: 'pv4', container, sandbox, schema });
        inst.load({});
        container.querySelector('#ext-ctrl-pv4-__setup_client_id').value = 'half-typed';
        container.querySelector('#ext-ctrl-pv4-next').click();
        await new Promise((r) => setTimeout(r, 0));
        expect(sandbox.calls.some((c) => c.method === 'getSettings')).toBe(true);
        expect(container.querySelector('#ext-ctrl-pv4-__setup_client_id').value).toBe('half-typed');
    });
});

describe('manifest defaults', () => {
    let container;

    beforeEach(() => {
        container = document.createElement('div');
        document.body.appendChild(container);
    });

    const manifest = {
        config: {
            enabled: { type: 'boolean', default: true },
            precision: { type: 'number', default: 2 },
            trigger: { type: 'string', default: 'calc' },
        },
    };

    function render(extensionId, controls, { logger, sandbox = stubSandbox() } = {}) {
        return renderSchema({
            extensionId,
            container,
            sandbox,
            manifest,
            logger: logger || { warn: vi.fn() },
            schema: { sections: [{ controls }] },
        });
    }

    it("uses the manifest default, not the control's, for a declared id", () => {
        const inst = render('md1', [
            { type: 'number', id: 'precision', label: 'P', default: 0 },
            { type: 'text', id: 'trigger', label: 'T' }, // no control default at all
        ]);
        inst.load({});
        expect(container.querySelector('#ext-ctrl-md1-precision').value).toBe('2');
        expect(container.querySelector('#ext-ctrl-md1-trigger').value).toBe('calc');
    });

    it('an emptied number field falls back to the manifest default', () => {
        const inst = render('md2', [{ type: 'number', id: 'precision', label: 'P', default: 0 }]);
        inst.load({ precision: 5 });
        container.querySelector('#ext-ctrl-md2-precision').value = '';
        expect(inst.currentValues().precision).toBe(2);
    });

    it('keeps the control default for ids the manifest does not declare', () => {
        const inst = render('md3', [
            { type: 'text', id: 'legacy', label: 'L', default: 'own' },
            { type: 'text', id: '__scratch', label: 'S', default: 'scratch' },
        ]);
        inst.load({});
        expect(container.querySelector('#ext-ctrl-md3-legacy').value).toBe('own');
        expect(container.querySelector('#ext-ctrl-md3-__scratch').value).toBe('scratch');
    });

    it('warns once per extension and id when a control default disagrees', async () => {
        const warn = vi.fn();
        const controls = [
            { type: 'number', id: 'precision', label: 'P', default: 0 },
            // Same value once shown: no warning.
            { type: 'text', id: 'trigger', label: 'T', default: 'calc' },
        ];
        const sandbox = stubSandbox({ getSettings: () => ({ sections: [{ controls }] }) });
        const inst = render('md4', controls, { logger: { warn }, sandbox });
        await inst.refresh();
        render('md4', controls, { logger: { warn } });
        expect(warn).toHaveBeenCalledTimes(1);
        expect(warn.mock.calls[0][0]).toMatch(/md4.*precision.*0.*manifest config default 2/);
        // A different extension with the same mismatch gets its own warning.
        render('md4b', controls, { logger: { warn } });
        expect(warn).toHaveBeenCalledTimes(2);
    });

    it('does not modify the schema object the extension returned', () => {
        const ctrl = { type: 'number', id: 'precision', label: 'P', default: 0 };
        render('md5', [ctrl]);
        expect(ctrl.default).toBe(0);
    });

    it('save() is sparse and keeps stored keys the page has no control for', () => {
        const inst = render('md6', [
            { type: 'number', id: 'precision', label: 'P', default: 0 },
            { type: 'text', id: 'trigger', label: 'T', default: 'calc' },
        ]);
        const stored = { precision: 4, other_page: true, __stale: 'x' };
        inst.load({ ...stored, trigger: 'calc' });
        const t = container.querySelector('#ext-ctrl-md6-trigger');
        t.value = '=';
        t.dispatchEvent(new Event('input'));
        expect(inst.save(stored)).toEqual({
            precision: 4,
            other_page: true,
            __stale: 'x',
            trigger: '=',
        });

        const p = container.querySelector('#ext-ctrl-md6-precision');
        p.value = '2';
        p.dispatchEvent(new Event('input'));
        t.value = 'calc';
        t.dispatchEvent(new Event('input'));
        // Back to the defaults: none of this page's keys are stored.
        expect(inst.save(stored)).toEqual({ other_page: true, __stale: 'x' });
    });
});

describe('the enabled control', () => {
    let container;

    beforeEach(() => {
        container = document.createElement('div');
        document.body.appendChild(container);
    });

    const controls = [
        { type: 'checkbox', id: 'enabled', label: 'Enable', default: true },
        { type: 'text', id: 'trigger', label: 'Trigger', default: 'sp' },
        { type: 'text', id: 'only_when_on', label: 'X', showWhen: { id: 'enabled', equals: true } },
    ];

    function render(extensionId) {
        return renderSchema({
            extensionId,
            container,
            sandbox: stubSandbox(),
            manifest: { config: { enabled: { type: 'boolean', default: true } } },
            schema: { sections: [{ controls }] },
        });
    }

    it('is never shown: the section header switch replaces it', () => {
        const inst = render('en1');
        inst.load({ enabled: true, trigger: 'sp' });
        expect(container.querySelector('#ext-row-en1-enabled').style.display).toBe('none');
        expect(container.querySelector('#ext-row-en1-trigger').style.display).toBe('');
    });

    it('holds the host state, which showWhen clauses follow', () => {
        const inst = render('en2');
        inst.load({ enabled: false, trigger: 'sp' });
        const row = container.querySelector('#ext-row-en2-only_when_on');
        expect(row.style.display).toBe('none');
        expect(inst.currentValues().enabled).toBe(false);

        inst.setHostEnabled(true);
        expect(row.style.display).toBe('');
        expect(container.querySelector('#ext-row-en2-enabled').style.display).toBe('none');
        expect(inst.currentValues().enabled).toBe(true);
    });

    it('is never saved, and a stale saved value is removed', () => {
        const inst = render('en3');
        inst.load({ enabled: false, trigger: 'x' });
        expect(inst.save({ enabled: false, trigger: 'x' })).toEqual({
            trigger: 'x',
            only_when_on: '',
        });
    });

    it('setHostEnabled is a no-op for a page without one', () => {
        const inst = renderSchema({
            extensionId: 'en4',
            container,
            sandbox: stubSandbox(),
            schema: { sections: [{ controls: [{ type: 'text', id: 'a', label: 'A' }] }] },
        });
        inst.load({});
        expect(() => inst.setHostEnabled(false)).not.toThrow();
        expect(inst.currentValues()).toEqual({ a: '' });
    });
});

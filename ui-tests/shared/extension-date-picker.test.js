/**
 * The extension side of the host date picker — the security boundary:
 *
 *   - hostFeaturesFor / ExtensionSandbox only offer 'date-picker' to
 *     extensions holding the `calendar` grant, and only claim a date
 *     provider when the window offered it;
 *   - the sanitizer keeps `data-kage-date` only as a real ISO day on a
 *     <button>;
 *   - _wireDateAffordances wires openers for eligible providers and unwraps
 *     every other affordance to inert text;
 *   - fetchDateMarkers validates the provider's payload;
 *   - resolveDateQuery only accepts a single-line query that starts with
 *     one of the provider's OWN keywords;
 *   - get_calendar_busy_days is a `calendar` command.
 *
 * Sandboxes are stubbed (a real null-origin iframe isn't testable in jsdom),
 * same approach as extension-manager-dom.test.js.
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';
import { initI18n } from '../../ui/js/shared/i18n.js';
import {
    ExtensionSandbox,
    HOST_FEATURE_DATE_PICKER,
    hostFeaturesFor,
} from '../../ui/js/shared/extension-sandbox-host.js';
import { sanitizeExtensionHtml } from '../../ui/js/shared/extension-html-sanitizer.js';
import { decideInvoke } from '../../ui/js/shared/extension-permissions.js';

function stubSandbox(responses = {}, flags = {}) {
    return {
        hasSearch: true,
        hasDateProvider: true,
        widgetIds: [],
        call: vi.fn(async (method, params) => {
            const r = responses[method];
            return typeof r === 'function' ? r(params) : r;
        }),
        updateConfig: vi.fn(),
        destroy: vi.fn(),
        ...flags,
    };
}

function manager(exts) {
    const mgr = new ExtensionManager(async () => undefined, { hostFeatures: [HOST_FEATURE_DATE_PICKER] });
    mgr._configCache = { extension_states: {}, extension_grants: {}, extensions: {} };
    for (const { id, capabilities = ['calendar'], sandbox, enabled = true } of exts) {
        mgr._configCache.extension_states[id] = enabled;
        mgr.extensions.set(id, { manifest: { id, name: id, permissions: capabilities }, capabilities, sandbox });
    }
    return mgr;
}

beforeAll(async () => {
    const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
    const catalog = JSON.parse(readFileSync(path.join(repoRoot, 'locales/en/messages.json'), 'utf8'));
    await initI18n(async () => ({ catalog, fallback: catalog, language: 'en' }));
});

const calendarKeywords = () => [
    { keyword: 'cal', labelKey: 'k' },
    { keyword: 'calendar', labelKey: 'k' },
];

describe('host feature gating', () => {
    it("offers 'date-picker' only with the calendar capability", () => {
        expect(hostFeaturesFor(['date-picker'], ['calendar', 'urls'])).toEqual(['date-picker']);
        expect(hostFeaturesFor(['date-picker'], ['urls'])).toEqual([]);
        expect(hostFeaturesFor(['date-picker', 'teleport'], ['calendar'])).toEqual(['date-picker']);
        expect(hostFeaturesFor(undefined, ['calendar'])).toEqual([]);
    });

    it('the sandbox re-filters what it was handed', () => {
        const sb = new ExtensionSandbox(
            { extensionId: 'x', capabilities: ['urls'], hostFeatures: ['date-picker'] },
            vi.fn(),
            document.body
        );
        expect(sb.hostFeatures).toEqual([]);
        const ok = new ExtensionSandbox(
            { extensionId: 'y', capabilities: ['calendar'], hostFeatures: ['date-picker'] },
            vi.fn(),
            document.body
        );
        expect(ok.hostFeatures).toEqual(['date-picker']);
    });

    it('forwards an AppError key/kind so a provider can spot a denied permission', async () => {
        const rawInvoke = vi.fn().mockRejectedValue({
            kind: 'internal',
            key: 'errors.calendar.access_denied',
            message: 'Calendar access denied',
        });
        const sb = new ExtensionSandbox({ extensionId: 'calendar', capabilities: ['calendar'] }, rawInvoke, document.body);
        const sent = [];
        sb._port = { postMessage: (m) => sent.push(m), close() {} };
        sb._ready = true;
        await sb._handleInvoke({ id: 7, command: 'get_calendar_busy_days', args: { start: '2026-10-26', end: '2026-12-07' } });
        expect(rawInvoke).toHaveBeenCalledWith('get_calendar_busy_days', { start: '2026-10-26', end: '2026-12-07' });
        expect(sent[0]).toEqual({
            type: 'invoke-response',
            id: 7,
            error: 'Calendar access denied',
            errorKey: 'errors.calendar.access_denied',
            errorKind: 'internal',
        });
    });

    it('get_calendar_busy_days needs the calendar capability', () => {
        expect(decideInvoke('get_calendar_busy_days', new Set(['calendar'])).allow).toBe(true);
        expect(decideInvoke('get_calendar_busy_days', new Set(['urls'])).allow).toBe(false);
    });
});

describe('sanitizer: data-kage-date', () => {
    const html = (s) => {
        const div = document.createElement('div');
        div.appendChild(sanitizeExtensionHtml(s, 'rich'));
        return div;
    };

    it('keeps a real ISO day on a button', () => {
        const el = html('<button data-kage-date="2026-11-26">Thu</button>').querySelector('button');
        expect(el.getAttribute('data-kage-date')).toBe('2026-11-26');
    });

    it('drops it on non-buttons and for anything but a real date', () => {
        const root = html(
            '<span data-kage-date="2026-11-26">a</span>' +
                '<button data-kage-date="2026-02-30">b</button>' +
                '<button data-kage-date="2026-11-26&quot; onclick=&quot;x">c</button>' +
                '<button data-kage-date="tomorrow">d</button>'
        );
        expect(root.querySelectorAll('[data-kage-date]')).toHaveLength(0);
        expect(root.querySelector('[onclick]')).toBeNull();
    });
});

describe('_wireDateAffordances', () => {
    const rendered = (markup) => {
        const root = document.createElement('div');
        root.appendChild(sanitizeExtensionHtml(markup, 'rich'));
        document.body.appendChild(root);
        return root;
    };

    it('wires an opener for an eligible provider, without firing the row underneath', () => {
        const mgr = manager([{ id: 'calendar', sandbox: stubSandbox() }]);
        const handler = vi.fn();
        mgr.setDateAffordanceHandler(handler);
        const row = rendered('<div><button data-kage-date="2026-11-26">[Thu, Nov 26]</button> 10:00</div>');
        const rowClick = vi.fn();
        row.addEventListener('click', rowClick);
        mgr._wireDateAffordances('calendar', row);
        const btn = row.querySelector('button');
        expect(btn.classList.contains('kage-date-affordance')).toBe(true);
        expect(btn.getAttribute('aria-haspopup')).toBe('dialog');
        expect(btn.getAttribute('aria-label')).toContain('[Thu, Nov 26]');
        btn.click();
        expect(handler).toHaveBeenCalledWith({ anchor: btn, date: '2026-11-26', extensionId: 'calendar' });
        expect(rowClick).not.toHaveBeenCalled();
        // Re-wiring the same DOM (re-render) doesn't stack listeners.
        mgr._wireDateAffordances('calendar', row);
        btn.click();
        expect(handler).toHaveBeenCalledTimes(2);
    });

    it.each([
        ['no calendar grant', [{ id: 'x', capabilities: ['urls'], sandbox: stubSandbox() }], true],
        ['not a date provider', [{ id: 'x', sandbox: stubSandbox({}, { hasDateProvider: false }) }], true],
        ['disabled', [{ id: 'x', sandbox: stubSandbox(), enabled: false }], true],
        ['window has no picker', [{ id: 'x', sandbox: stubSandbox() }], false],
    ])('unwraps to inert text when %s', (_label, exts, withHandler) => {
        const mgr = manager(exts);
        if (withHandler) mgr.setDateAffordanceHandler(vi.fn());
        const row = rendered('<div><button class="cal" data-kage-date="2026-11-26">Thu</button></div>');
        mgr._wireDateAffordances('x', row);
        expect(row.querySelector('button')).toBeNull();
        expect(row.querySelector('span.cal').textContent).toBe('Thu');
    });
});

describe('date provider lookups', () => {
    it('getDateProvider picks the first eligible extension', () => {
        const mgr = manager([
            { id: 'notes', capabilities: ['storage'], sandbox: stubSandbox() },
            { id: 'calendar', sandbox: stubSandbox() },
        ]);
        expect(mgr.getDateProvider()).toBe('calendar');
        expect(manager([{ id: 'a', capabilities: [], sandbox: stubSandbox() }]).getDateProvider()).toBeNull();
    });

    it('fetchDateMarkers validates the payload and never throws', async () => {
        const mgr = manager([
            {
                id: 'calendar',
                sandbox: stubSandbox({
                    getDateMarkers: ({ start, end }) => ({
                        status: 'ok',
                        days: [
                            { date: '2026-11-05', count: 2 },
                            { date: '2027-05-05', count: 1 }, // outside the range
                            { date: 'x', count: 1 },
                        ],
                        _range: [start, end],
                    }),
                }),
            },
            { id: 'broken', sandbox: stubSandbox({ getDateMarkers: () => { throw new Error('boom'); } }) },
        ]);
        const m = await mgr.fetchDateMarkers('calendar', '2026-10-26', '2026-12-07');
        expect([...m.days.keys()]).toEqual(['2026-11-05']);
        expect(mgr.extensions.get('calendar').sandbox.call).toHaveBeenCalledWith('getDateMarkers', {
            start: '2026-10-26',
            end: '2026-12-07',
        });
        expect((await mgr.fetchDateMarkers('broken', '2026-10-26', '2026-12-07')).status).toBe('error');
        expect((await mgr.fetchDateMarkers('nobody', '2026-10-26', '2026-12-07')).status).toBe('no_provider');
    });

    it('resolveDateQuery accepts only a query under the provider’s own keywords', async () => {
        const answers = {
            '2026-11-26': { query: 'cal 2026-11-26' },
            '2026-11-27': { query: 'spotify play loud' }, // someone else's keyword
            '2026-11-28': { query: 'cal 2026-11-28\nrm -rf /' },
            '2026-11-29': { query: 'calx 2026-11-29' }, // prefix, not a keyword
            '2026-11-30': null,
        };
        const sandbox = stubSandbox({
            onDateSelected: ({ date }) => answers[date],
            getKeywords: calendarKeywords,
        });
        const mgr = manager([
            { id: 'calendar', sandbox },
            { id: 'spotify', capabilities: [], sandbox: stubSandbox({ getKeywords: () => [{ keyword: 'spotify' }] }) },
        ]);
        expect(await mgr.resolveDateQuery('calendar', '2026-11-26')).toBe('cal 2026-11-26');
        expect(await mgr.resolveDateQuery('calendar', '2026-11-27')).toBeNull();
        expect(await mgr.resolveDateQuery('calendar', '2026-11-28')).toBeNull();
        expect(await mgr.resolveDateQuery('calendar', '2026-11-29')).toBeNull();
        expect(await mgr.resolveDateQuery('calendar', '2026-11-30')).toBeNull();
        // Invalid dates never reach the sandbox.
        sandbox.call.mockClear();
        expect(await mgr.resolveDateQuery('calendar', '2026-02-30')).toBeNull();
        expect(sandbox.call).not.toHaveBeenCalled();
    });
});

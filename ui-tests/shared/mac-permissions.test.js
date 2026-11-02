/**
 * Tests for ui/js/shared/mac-permissions.js — the macOS TCC permission
 * cards used by the welcome wizard and the macOS-only settings pane.
 *
 * Behaviour we lock in:
 *   - isMacOS() reads from navigator.platform
 *   - MAC_PERMISSIONS lists the permissions Kage actually uses
 *     (Accessibility, Screen Recording — no Input Monitoring), frozen,
 *     with localised name/why
 *   - renderPermissionCard() escapes HTML and shows Repair + its hint
 *     only while a permission is not granted
 *   - applyStatus() updates a card in place without re-announcing
 *     unchanged text
 *   - renderAllInto() fetches live status, refreshes on window focus,
 *     ignores stale responses, wires open_url, and runs the repair flow
 *     (repair → open the pane → message), reporting failures
 */

import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import {
    MAC_PERMISSIONS,
    PERMISSION_STATUS,
    applyStatus,
    isMacOS,
    normalizeStatus,
    renderAllInto,
    renderPermissionCard,
    statusLabel,
} from '../../ui/js/shared/mac-permissions.js';
import { initI18n } from '../../ui/js/shared/i18n.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const catalog = JSON.parse(
    readFileSync(join(__dirname, '..', '..', 'locales', 'en', 'messages.json'), 'utf-8')
);
const en = (key) => catalog[key].message;

// Real EN strings instead of literal keys, so assertions read like the UI.
beforeAll(async () => {
    await initI18n(async (cmd) => {
        if (cmd === 'get_i18n_catalog') {
            return { language: 'en', rtl: false, catalog, fallback: catalog };
        }
        throw new Error(`unexpected invoke: ${cmd}`);
    });
});

beforeEach(() => {
    document.body.innerHTML = '';
});

const flush = async () => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

/**
 * Fake `invoke` backed by a mutable status table. `repair` decides what
 * repair_privacy_permission does.
 */
function fakeBackend(initial, { repair } = {}) {
    const statuses = { ...initial };
    const invoke = vi.fn(async (cmd, args) => {
        switch (cmd) {
            case 'get_privacy_permissions':
                return Object.entries(statuses).map(([id, status]) => ({ id, status }));
            case 'open_url':
                return undefined;
            case 'repair_privacy_permission':
                if (repair) return repair(args);
                return { id: args.permission, status: 'not_granted' };
            default:
                throw new Error(`unexpected invoke: ${cmd}`);
        }
    });
    return { invoke, statuses };
}

function mount(invoke, prefix = 'wMacPerm') {
    const container = document.createElement('div');
    document.body.appendChild(container);
    const controller = renderAllInto(container, invoke, prefix);
    return { container, controller };
}

const card = (container, id) => container.querySelector(`.mac-perm-card[data-perm-id="${id}"]`);
const pillText = (el) => el.querySelector('[data-role="status-label"]').textContent;
const repairBtn = (el) => el.querySelector('[data-role="repair"]');
const hint = (el) => el.querySelector('[data-role="repair-hint"]');
const message = (el) => el.querySelector('[data-role="message"]');

describe('isMacOS', () => {
    it('returns true on MacIntel', () => {
        Object.defineProperty(navigator, 'platform', {
            value: 'MacIntel',
            configurable: true,
        });
        expect(isMacOS()).toBe(true);
    });

    it('returns false on Win32', () => {
        Object.defineProperty(navigator, 'platform', {
            value: 'Win32',
            configurable: true,
        });
        expect(isMacOS()).toBe(false);
    });

    it('returns false on Linux x86_64', () => {
        Object.defineProperty(navigator, 'platform', {
            value: 'Linux x86_64',
            configurable: true,
        });
        expect(isMacOS()).toBe(false);
    });
});

describe('MAC_PERMISSIONS', () => {
    it('lists the permissions Kage uses on macOS (no Input Monitoring)', () => {
        // Global hotkeys go through Carbon RegisterEventHotKey, which needs
        // no permission — Input Monitoring must not come back as a card.
        const ids = MAC_PERMISSIONS.map((p) => p.id);
        expect(ids).toEqual(['accessibility', 'screen-recording']);
    });

    it('is frozen so callers cannot mutate the canonical list', () => {
        // The constant is the source of truth for the welcome wizard
        // and the settings pane — mutating it would silently rewrite
        // both surfaces.
        expect(Object.isFrozen(MAC_PERMISSIONS)).toBe(true);
        expect(Object.isFrozen(MAC_PERMISSIONS[0])).toBe(true);
    });

    it('every permission ships an x-apple deep link', () => {
        for (const p of MAC_PERMISSIONS) {
            expect(p.url.startsWith('x-apple.systempreferences:')).toBe(true);
        }
    });

    it('resolves name and why through the i18n catalog', () => {
        const [ax, screen] = MAC_PERMISSIONS;
        expect(ax.name).toBe(en('shared.mac_permissions.accessibility.name'));
        expect(ax.why).toBe(en('shared.mac_permissions.accessibility.why'));
        expect(screen.name).toBe(en('shared.mac_permissions.screen_recording.name'));
        expect(screen.why).toBe(en('shared.mac_permissions.screen_recording.why'));
    });
});

describe('normalizeStatus / statusLabel', () => {
    it('keeps known card states', () => {
        for (const s of ['checking', 'granted', 'not_granted']) {
            expect(normalizeStatus(s)).toBe(s);
        }
    });

    it('collapses anything else (incl. the non-mac not_applicable) to unknown', () => {
        for (const s of ['not_applicable', undefined, null, '', 'GRANTED', 42]) {
            expect(normalizeStatus(s)).toBe(PERMISSION_STATUS.UNKNOWN);
        }
    });

    it('labels each state from the catalog', () => {
        expect(statusLabel('granted')).toBe('Granted');
        expect(statusLabel('not_granted')).toBe('Not granted');
        expect(statusLabel('checking')).toBe(en('shared.mac_permissions.status.checking'));
        expect(statusLabel('whatever')).toBe('Unknown');
    });
});

describe('renderPermissionCard', () => {
    it('renders the name, the open button id, and a checking pill by default', () => {
        const html = renderPermissionCard(MAC_PERMISSIONS[0], 'btn-1');
        expect(html).toContain('Accessibility');
        expect(html).toContain('id="btn-1-btn"');
        document.body.innerHTML = html;
        const el = document.querySelector('.mac-perm-card');
        expect(el.dataset.status).toBe('checking');
        expect(pillText(el)).toBe(en('shared.mac_permissions.status.checking'));
        expect(repairBtn(el).hidden).toBe(true);
    });

    it('shows Repair and its explanation only while not granted', () => {
        document.body.innerHTML = renderPermissionCard(MAC_PERMISSIONS[1], 'x', 'not_granted');
        const el = document.querySelector('.mac-perm-card');
        expect(repairBtn(el).hidden).toBe(false);
        expect(hint(el).hidden).toBe(false);
        expect(hint(el).textContent).toBe(en('shared.mac_permissions.repair_hint'));
        // Repair is described by the hint and labelled with the permission.
        expect(repairBtn(el).getAttribute('aria-describedby')).toBe(hint(el).id);
        expect(repairBtn(el).getAttribute('aria-label')).toBe('Repair Screen Recording permission');

        document.body.innerHTML = renderPermissionCard(MAC_PERMISSIONS[1], 'x', 'granted');
        const granted = document.querySelector('.mac-perm-card');
        expect(repairBtn(granted).hidden).toBe(true);
        expect(hint(granted).hidden).toBe(true);
    });

    it('exposes the status as a polite live region with the permission name', () => {
        document.body.innerHTML = renderPermissionCard(MAC_PERMISSIONS[0], 'x', 'granted');
        const pill = document.querySelector('[data-role="status"]');
        expect(pill.getAttribute('role')).toBe('status');
        expect(pill.getAttribute('aria-live')).toBe('polite');
        expect(pill.textContent).toBe('Accessibility: Granted');
    });

    it('escapes HTML in name and why so localized strings cannot break markup', () => {
        // A localizer who sneaks `<script>` into the copy would otherwise
        // ship live JS into every install via System Settings copy.
        // Verify both the body fields (name/why) and the attribute
        // fields (id, idBase) sanitise their inputs.
        const evil = {
            id: 'evil-id',
            icon: 'X',
            name: '<script>alert(1)</script>',
            why: '"><img src=x onerror=alert(2)>',
            url: 'x-apple:foo',
        };
        const html = renderPermissionCard(evil, 'btn"id');
        // Name: `<` is escaped, so the inline script never forms a tag.
        expect(html).not.toContain('<script>alert(1)');
        expect(html).toContain('&lt;script&gt;');
        // Why: `<img>` is also entity-encoded — no element forms, so
        // onerror never fires. The `"` in body text is harmless and
        // is left as-is by escapeHtml (only escapeAttr escapes quotes).
        expect(html).toContain('&lt;img src=x onerror=alert(2)&gt;');
        // idBase lands inside `id="..."` — escapeAttr quote-encodes
        // the embedded `"` so the attribute can't be closed early.
        expect(html).toContain('id="btn&quot;id-btn"');
        // The name also feeds the aria-labels; those must stay inside
        // their attribute too.
        expect(html).not.toContain('aria-label="Repair <script>');
    });
});

describe('applyStatus', () => {
    it('updates the pill and toggles Repair in place', () => {
        document.body.innerHTML = renderPermissionCard(MAC_PERMISSIONS[0], 'x');
        const el = document.querySelector('.mac-perm-card');

        applyStatus(el, 'not_granted');
        expect(el.dataset.status).toBe('not_granted');
        expect(el.querySelector('[data-role="status"]').dataset.status).toBe('not_granted');
        expect(pillText(el)).toBe('Not granted');
        expect(repairBtn(el).hidden).toBe(false);

        applyStatus(el, 'granted');
        expect(pillText(el)).toBe('Granted');
        expect(repairBtn(el).hidden).toBe(true);
        expect(hint(el).hidden).toBe(true);

        applyStatus(el, 'not_applicable');
        expect(pillText(el)).toBe('Unknown');
        expect(repairBtn(el).hidden).toBe(true);
    });

    it('leaves unchanged text alone so screen readers do not re-announce it', () => {
        document.body.innerHTML = renderPermissionCard(MAC_PERMISSIONS[0], 'x', 'granted');
        const el = document.querySelector('.mac-perm-card');
        const textNode = el.querySelector('[data-role="status-label"]').firstChild;
        applyStatus(el, 'granted');
        expect(el.querySelector('[data-role="status-label"]').firstChild).toBe(textNode);
    });
});

describe('renderAllInto', () => {
    it('renders one card per permission and wires open_url on click', async () => {
        const { invoke } = fakeBackend({ accessibility: 'granted', 'screen-recording': 'granted' });
        const { container } = mount(invoke);
        await flush();

        const buttons = container.querySelectorAll('button[data-role="open"]');
        expect(buttons.length).toBe(MAC_PERMISSIONS.length);

        // Click the first card's button — should invoke open_url with
        // the deep link from the matching MAC_PERMISSIONS entry.
        invoke.mockClear();
        buttons[0].click();
        expect(invoke).toHaveBeenCalledTimes(1);
        const [cmd, args] = invoke.mock.calls[0];
        expect(cmd).toBe('open_url');
        expect(args.url).toBe(MAC_PERMISSIONS[0].url);
    });

    it('shows live status from get_privacy_permissions', async () => {
        const { invoke } = fakeBackend({
            accessibility: 'not_granted',
            // Reported by the backend but not shown as a card.
            'input-monitoring': 'not_granted',
            'screen-recording': 'granted',
        });
        const { container } = mount(invoke);
        expect(pillText(card(container, 'accessibility'))).toBe(
            en('shared.mac_permissions.status.checking')
        );
        await flush();

        expect(invoke).toHaveBeenCalledWith('get_privacy_permissions');
        const ax = card(container, 'accessibility');
        expect(pillText(ax)).toBe('Not granted');
        expect(repairBtn(ax).hidden).toBe(false);
        const screen = card(container, 'screen-recording');
        expect(pillText(screen)).toBe('Granted');
        expect(repairBtn(screen).hidden).toBe(true);
        expect(container.querySelector('[data-perm-id="input-monitoring"]')).toBeNull();
    });

    it('falls back to Unknown (no Repair) when the status check fails', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const invoke = vi.fn(async () => {
            throw { kind: 'internal', message: 'boom' };
        });
        const { container } = mount(invoke);
        await flush();
        for (const p of MAC_PERMISSIONS) {
            expect(pillText(card(container, p.id))).toBe('Unknown');
            expect(repairBtn(card(container, p.id)).hidden).toBe(true);
        }
        warn.mockRestore();
    });

    it('refreshes when the window regains focus', async () => {
        const backend = fakeBackend({ accessibility: 'not_granted', 'screen-recording': 'granted' });
        const { container } = mount(backend.invoke);
        await flush();
        expect(pillText(card(container, 'accessibility'))).toBe('Not granted');

        // User flips the switch in System Settings and comes back.
        backend.statuses.accessibility = 'granted';
        window.dispatchEvent(new Event('focus'));
        await flush();
        expect(pillText(card(container, 'accessibility'))).toBe('Granted');
        expect(repairBtn(card(container, 'accessibility')).hidden).toBe(true);
    });

    it('stops refreshing on focus after dispose()', async () => {
        const backend = fakeBackend({ accessibility: 'not_granted', 'screen-recording': 'granted' });
        const { controller } = mount(backend.invoke);
        await flush();
        controller.dispose();
        backend.invoke.mockClear();
        window.dispatchEvent(new Event('focus'));
        await flush();
        expect(backend.invoke).not.toHaveBeenCalled();
    });

    it('drops its focus listener once the container leaves the DOM', async () => {
        const backend = fakeBackend({ accessibility: 'granted', 'screen-recording': 'granted' });
        const { container } = mount(backend.invoke);
        await flush();
        container.remove();
        backend.invoke.mockClear();
        window.dispatchEvent(new Event('focus'));
        window.dispatchEvent(new Event('focus'));
        await flush();
        expect(backend.invoke).not.toHaveBeenCalled();
    });

    it('ignores a stale status response that lands after a newer one', async () => {
        let resolveFirst;
        let call = 0;
        const invoke = vi.fn((cmd) => {
            if (cmd !== 'get_privacy_permissions') return Promise.resolve();
            call += 1;
            if (call === 1) {
                return new Promise((r) => {
                    resolveFirst = () => r([{ id: 'accessibility', status: 'not_granted' }]);
                });
            }
            return Promise.resolve([{ id: 'accessibility', status: 'granted' }]);
        });
        const { container, controller } = mount(invoke);
        await controller.refresh();
        resolveFirst();
        await flush();
        expect(pillText(card(container, 'accessibility'))).toBe('Granted');
    });

    it('repairs: resets, opens the matching pane, explains the next step, refreshes', async () => {
        const backend = fakeBackend({ accessibility: 'granted', 'screen-recording': 'not_granted' });
        const { container } = mount(backend.invoke);
        await flush();
        backend.invoke.mockClear();

        const screen = card(container, 'screen-recording');
        repairBtn(screen).click();
        // Busy while the backend works; a double click can't start a second run.
        expect(repairBtn(screen).disabled).toBe(true);
        expect(repairBtn(screen).getAttribute('aria-busy')).toBe('true');
        repairBtn(screen).click();
        await flush();

        const calls = backend.invoke.mock.calls.map(([cmd, args]) => [cmd, args]);
        expect(calls.filter(([cmd]) => cmd === 'repair_privacy_permission')).toEqual([
            ['repair_privacy_permission', { permission: 'screen-recording' }],
        ]);
        const repairIdx = calls.findIndex(([cmd]) => cmd === 'repair_privacy_permission');
        const openIdx = calls.findIndex(([cmd]) => cmd === 'open_url');
        expect(openIdx).toBeGreaterThan(repairIdx);
        expect(calls[openIdx][1]).toEqual({ url: MAC_PERMISSIONS[1].url });
        // Status re-read after the action.
        expect(calls.slice(openIdx).some(([cmd]) => cmd === 'get_privacy_permissions')).toBe(true);

        expect(message(screen).textContent).toBe(en('shared.mac_permissions.repair_done'));
        expect(message(screen).classList.contains('error')).toBe(false);
        expect(repairBtn(screen).disabled).toBe(false);
        expect(repairBtn(screen).hasAttribute('aria-busy')).toBe(false);
        expect(repairBtn(screen).textContent).toBe(en('shared.mac_permissions.repair'));
    });

    it('reports a failed repair and does not open System Settings', async () => {
        const error = vi.spyOn(console, 'error').mockImplementation(() => {});
        const backend = fakeBackend(
            { accessibility: 'not_granted', 'screen-recording': 'granted' },
            {
                repair: () => {
                    throw { kind: 'internal', message: 'Repair only works in the installed Kage app' };
                },
            }
        );
        const { container } = mount(backend.invoke);
        await flush();
        backend.invoke.mockClear();

        const ax = card(container, 'accessibility');
        repairBtn(ax).click();
        await flush();

        expect(backend.invoke.mock.calls.some(([cmd]) => cmd === 'open_url')).toBe(false);
        expect(message(ax).textContent).toBe(
            'Repair failed: Repair only works in the installed Kage app'
        );
        expect(message(ax).classList.contains('error')).toBe(true);
        expect(repairBtn(ax).disabled).toBe(false);
        error.mockRestore();
    });

    it('uses default "macPerm" prefix when none is supplied', () => {
        const invoke = vi.fn().mockResolvedValue([]);
        const container = document.createElement('div');
        renderAllInto(container, invoke);
        expect(container.querySelector('#macPerm-accessibility-btn')).not.toBeNull();
        expect(container.querySelector('#macPerm-accessibility-repair')).not.toBeNull();
    });
});

/**
 * Tests for ui/js/shared/mac-permissions.js — the macOS TCC permission
 * UX helpers used by the welcome wizard and the macOS-only settings
 * pane.
 *
 * Behaviour we lock in:
 *   - isMacOS() reads from navigator.platform
 *   - MAC_PERMISSIONS exposes the three required permissions, each
 *     frozen so callers can't mutate the canonical list at runtime
 *   - renderPermissionCard() escapes HTML in name/why fields so a
 *     localized string with `<` characters can't break out of markup
 *   - renderAllInto() wires per-card buttons to invoke('open_url')
 *     with the matching x-apple deep link
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
    MAC_PERMISSIONS,
    isMacOS,
    renderAllInto,
    renderPermissionCard,
} from '../../ui/js/shared/mac-permissions.js';

beforeEach(() => {
    document.body.innerHTML = '';
});

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
    it('lists the three permissions Kage requires on macOS', () => {
        const ids = MAC_PERMISSIONS.map((p) => p.id);
        expect(ids).toEqual(['accessibility', 'input-monitoring', 'screen-recording']);
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
});

describe('renderPermissionCard', () => {
    it('renders a card for the given permission with the supplied button id', () => {
        const html = renderPermissionCard(MAC_PERMISSIONS[0], 'btn-1');
        expect(html).toContain('Accessibility');
        expect(html).toContain('id="btn-1"');
    });

    it('escapes HTML in name and why so localized strings cannot break markup', () => {
        // A localizer who sneaks `<script>` into the copy would otherwise
        // ship live JS into every install via System Settings copy. The
        // mac-permissions.js source has its own `escapeHtml` for exactly
        // this — verify it kicks in.
        const evil = {
            id: 'evil',
            icon: 'X',
            name: '<script>alert(1)</script>',
            why: '"><img src=x onerror=alert(2)>',
            url: 'x-apple:foo',
        };
        const html = renderPermissionCard(evil, 'b');
        expect(html).not.toContain('<script>alert(1)');
        expect(html).toContain('&lt;script&gt;');
        expect(html).toContain('&quot;');
    });
});

describe('renderAllInto', () => {
    it('renders one card per permission and wires open_url on click', () => {
        const invoke = vi.fn().mockResolvedValue(undefined);
        const container = document.createElement('div');
        document.body.appendChild(container);

        renderAllInto(container, invoke, 'wMacPerm');

        const buttons = container.querySelectorAll('button.mac-perm-btn');
        expect(buttons.length).toBe(3);

        // Click the first card's button — should invoke open_url with
        // the deep link from the matching MAC_PERMISSIONS entry.
        buttons[0].click();
        expect(invoke).toHaveBeenCalledTimes(1);
        const [cmd, args] = invoke.mock.calls[0];
        expect(cmd).toBe('open_url');
        expect(args.url).toBe(MAC_PERMISSIONS[0].url);
    });

    it('uses default "macPerm" prefix when none is supplied', () => {
        const invoke = vi.fn();
        const container = document.createElement('div');
        renderAllInto(container, invoke);
        expect(container.querySelector('#macPerm-accessibility-btn')).not.toBeNull();
    });
});

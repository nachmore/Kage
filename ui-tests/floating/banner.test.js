import { describe, it, expect, beforeEach, vi } from 'vitest';
import { resolveBannerAction, BannerController } from '../../ui/js/floating/banner.js';

// i18n + session-render are imported by banner.js; stub them so the unit
// under test stays pure (t echoes the key; formatError stringifies).
vi.mock('../../ui/js/shared/i18n.js', () => ({ t: (k) => k }));
vi.mock('../../ui/js/shared/session-render.js', () => ({
    formatError: (e) => String(e),
}));

describe('resolveBannerAction', () => {
    it('maps null/unknown to dismiss', () => {
        expect(resolveBannerAction(null)).toEqual({ kind: 'dismiss' });
        expect(resolveBannerAction({ type: 'mystery' })).toEqual({ kind: 'dismiss' });
        expect(resolveBannerAction({})).toEqual({ kind: 'dismiss' });
    });

    it('parses a bare settings section', () => {
        expect(resolveBannerAction({ type: 'settings', data: 'updates' })).toEqual({
            kind: 'settings',
            section: 'updates',
        });
    });

    it('parses a settings section:subsection', () => {
        expect(resolveBannerAction({ type: 'settings', data: 'updates:changelog' })).toEqual({
            kind: 'settings',
            section: 'updates',
            subSection: 'changelog',
        });
    });

    it('defaults the settings section to updates when data is missing', () => {
        expect(resolveBannerAction({ type: 'settings' })).toEqual({
            kind: 'settings',
            section: 'updates',
        });
    });

    it('maps url and crash_log to their intents', () => {
        expect(resolveBannerAction({ type: 'url', data: 'https://x.test' })).toEqual({
            kind: 'url',
            url: 'https://x.test',
        });
        expect(resolveBannerAction({ type: 'crash_log', data: '/tmp/crash.log' })).toEqual({
            kind: 'open_path',
            path: '/tmp/crash.log',
        });
    });

    it('maps update_install', () => {
        expect(resolveBannerAction({ type: 'update_install' })).toEqual({ kind: 'install_update' });
    });
});

// Minimal banner DOM the controller pokes at.
function mountBannerDom() {
    document.body.innerHTML = `
        <div id="contentArea">
            <div id="floatingBanner" style="display:none">
                <span id="bannerIcon"></span>
                <span id="bannerText"></span>
                <span id="bannerAction"></span>
            </div>
            <div id="responseText"></div>
            <button id="expandBtn"></button>
        </div>
    `;
}

function makeController(overrides = {}) {
    return new BannerController({
        invoke: vi.fn(async () => {}),
        resizeWindow: vi.fn(),
        resetUI: vi.fn(),
        isWaitingForResponse: () => false,
        windowManager: { userSetHeight: 123 },
        ...overrides,
    });
}

describe('BannerController.show / dismiss', () => {
    beforeEach(() => {
        mountBannerDom();
        // jsdom lacks rAF in some configs; make it synchronous.
        vi.stubGlobal('requestAnimationFrame', (cb) => cb());
    });

    it('shows the banner with icon/text/action and marks visible', () => {
        const c = makeController();
        c.show('🎉', 'Updated!', 'View', 'settings', 'updates:changelog');
        expect(c.visible).toBe(true);
        expect(document.getElementById('floatingBanner').style.display).toBe('flex');
        expect(document.getElementById('bannerIcon').textContent).toBe('🎉');
        expect(document.getElementById('bannerText').innerHTML).toBe('Updated!');
        expect(document.getElementById('bannerAction').textContent).toBe('View');
        expect(document.getElementById('contentArea').classList.contains('visible')).toBe(true);
    });

    it('enters banner-only mode when there is no response content', () => {
        const c = makeController();
        c.show('💥', 'crashed', 'View log', 'crash_log', '/p');
        expect(document.getElementById('contentArea').classList.contains('banner-only')).toBe(true);
    });

    it('does NOT enter banner-only mode when a response is present', () => {
        document.getElementById('responseText').textContent = 'a streamed reply';
        const c = makeController();
        c.show('💥', 'crashed', 'View log', 'crash_log', '/p');
        expect(document.getElementById('contentArea').classList.contains('banner-only')).toBe(false);
    });

    it('dismiss hides the banner and collapses when idle + empty', () => {
        const c = makeController();
        c.show('🎉', 'Updated!', 'View', 'dismiss', '');
        c.dismiss();
        expect(c.visible).toBe(false);
        expect(document.getElementById('floatingBanner').style.display).toBe('none');
        expect(document.getElementById('contentArea').classList.contains('visible')).toBe(false);
    });

    it('dismiss keeps content area visible when a response is streaming', () => {
        const c = makeController({ isWaitingForResponse: () => true });
        c.show('🎉', 'Updated!', 'View', 'dismiss', '');
        c.dismiss();
        // Still hides the banner element, but does not collapse content.
        expect(document.getElementById('contentArea').classList.contains('visible')).toBe(true);
    });

    it('dismiss is a no-op when not visible', () => {
        const c = makeController();
        expect(() => c.dismiss()).not.toThrow();
        expect(c.visible).toBe(false);
    });
});

describe('BannerController.handleClick', () => {
    beforeEach(() => {
        mountBannerDom();
        vi.stubGlobal('requestAnimationFrame', (cb) => cb());
    });

    it('routes a settings click to open_settings_window with section args', async () => {
        const invoke = vi.fn(async () => {});
        const c = makeController({ invoke });
        c.show('🎉', 'Updated!', 'View', 'settings', 'updates:changelog');
        c.handleClick();
        expect(invoke).toHaveBeenCalledWith('open_settings_window', {
            section: 'updates',
            subSection: 'changelog',
        });
        expect(c.visible).toBe(false); // dismissed first
    });

    it('routes a crash_log click to open_path', () => {
        const invoke = vi.fn(async () => {});
        const c = makeController({ invoke });
        c.show('💥', 'crashed', 'View log', 'crash_log', '/tmp/c.log');
        c.handleClick();
        expect(invoke).toHaveBeenCalledWith('open_path', { path: '/tmp/c.log' });
    });

    it('routes a dismiss click to resetUI', () => {
        const resetUI = vi.fn();
        const c = makeController({ resetUI });
        c.show('🎉', 'x', '', 'dismiss', '');
        c.handleClick();
        expect(resetUI).toHaveBeenCalled();
    });

    it('install_update shows an installing banner then invokes the updater', () => {
        const invoke = vi.fn(async () => {});
        const c = makeController({ invoke });
        c.show('⬆️', 'Update available', 'Install', 'update_install', '');
        c.handleClick();
        // Re-shows an "installing" banner (so it stays visible) and calls the command.
        expect(c.visible).toBe(true);
        expect(invoke).toHaveBeenCalledWith('download_and_install_update');
    });
});

describe('BannerController.checkForUpdateBanner', () => {
    beforeEach(() => {
        mountBannerDom();
        vi.stubGlobal('requestAnimationFrame', (cb) => cb());
    });

    it('returns true and shows the banner when an update just landed', async () => {
        const invoke = vi.fn(async (cmd) => (cmd === 'was_just_updated' ? true : undefined));
        const c = makeController({ invoke });
        const shown = await c.checkForUpdateBanner();
        expect(shown).toBe(true);
        expect(c.visible).toBe(true);
        expect(invoke).toHaveBeenCalledWith('clear_update_flag');
    });

    it('returns false and shows nothing when there was no update', async () => {
        const invoke = vi.fn(async () => false);
        const c = makeController({ invoke });
        const shown = await c.checkForUpdateBanner();
        expect(shown).toBe(false);
        expect(c.visible).toBe(false);
    });
});

describe('BannerController.checkForCrashBanner', () => {
    beforeEach(() => {
        mountBannerDom();
        vi.stubGlobal('requestAnimationFrame', (cb) => cb());
    });

    it('shows a crash banner and acknowledges the crash', async () => {
        const invoke = vi.fn(async (cmd) =>
            cmd === 'get_recent_crash'
                ? { panic_message: 'boom', log_path: '/tmp/c.log', timestamp: 42 }
                : undefined
        );
        const c = makeController({ invoke });
        await c.checkForCrashBanner();
        expect(c.visible).toBe(true);
        expect(invoke).toHaveBeenCalledWith('dismiss_recent_crash', { timestamp: 42 });
    });

    it('does nothing when there is no crash', async () => {
        const invoke = vi.fn(async () => null);
        const c = makeController({ invoke });
        await c.checkForCrashBanner();
        expect(c.visible).toBe(false);
    });

    it('yields to an already-visible banner', async () => {
        const invoke = vi.fn(async (cmd) =>
            cmd === 'get_recent_crash' ? { log_path: '/p', timestamp: 1 } : undefined
        );
        const c = makeController({ invoke });
        c.visible = true; // e.g. the update banner is already up
        await c.checkForCrashBanner();
        // Should not have acknowledged the crash (it deferred).
        expect(invoke).not.toHaveBeenCalledWith('dismiss_recent_crash', expect.anything());
    });
});

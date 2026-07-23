/**
 * Tests for extensionDisplay() — the one helper every display site must
 * use to get an extension's user-facing name/icon.
 *
 * Regression context: installed manifests carry the raw Chrome-style
 * token `name: "__MSG_manifest.name__"`; the manager resolves it into
 * `ext.localizedManifest` at load time. Sites that read `manifest.name`
 * directly (the widget-paused notice did) leak the token to the UI:
 * "Widget paused — __MSG_manifest.name__ kept failing…".
 */

import { describe, it, expect, beforeAll, vi } from 'vitest';
import { extensionDisplay, ExtensionManager } from '../../ui/js/shared/extension-manager.js';
import { initI18n } from '../../ui/js/shared/i18n.js';

const TOKEN_MANIFEST = { id: 'spotify', name: '__MSG_manifest.name__', icon: '🎵' };

// The paused notice renders through tHtml(); seed the shared i18n module
// with the real EN template so the assertion exercises interpolation.
beforeAll(async () => {
    const catalog = {
        'shared.extension.widget.paused_html': {
            message:
                '<span>⚠️ Widget paused — <strong>{name}</strong> kept failing or running too slow.</span>',
        },
        'shared.extension.widget.retry': { message: 'Retry' },
    };
    await initI18n(async (cmd) => {
        if (cmd === 'get_i18n_catalog') {
            return { language: 'en', rtl: false, catalog, fallback: catalog };
        }
        throw new Error(`unexpected invoke: ${cmd}`);
    });
});

describe('extensionDisplay', () => {
    it('prefers the localized manifest name', () => {
        const ext = {
            manifest: TOKEN_MANIFEST,
            localizedManifest: { ...TOKEN_MANIFEST, name: 'Spotify' },
        };
        expect(extensionDisplay(ext, 'spotify').name).toBe('Spotify');
    });

    it('falls back to the wire manifest when nothing was localized', () => {
        const ext = { manifest: { id: 'x', name: 'Plain Name' } };
        expect(extensionDisplay(ext, 'x').name).toBe('Plain Name');
    });

    it('never returns a raw __MSG_*__ token', () => {
        // Localization failed (no _locales/, missing key) — the manager
        // stores the token unchanged in localizedManifest. The id is a
        // better label than the token.
        const ext = { manifest: TOKEN_MANIFEST, localizedManifest: TOKEN_MANIFEST };
        expect(extensionDisplay(ext, 'spotify').name).toBe('spotify');
    });

    it('uses the fallback id for a missing extension entry', () => {
        expect(extensionDisplay(undefined, 'ghost').name).toBe('ghost');
        expect(extensionDisplay(null, 'ghost').name).toBe('ghost');
    });
});

describe('widget paused notice name', () => {
    function makeTrippedManager(extEntry) {
        const mgr = new ExtensionManager(async () => undefined);
        mgr.extensions.set('spotify', extEntry);
        const host = document.createElement('div');
        const controller = {
            extensionId: 'spotify',
            widgetId: 'now-playing',
            slot: 'floating-bottom',
            host,
            timer: null,
            destroyed: false,
            refreshIntervalMs: 5000,
            renderInFlight: false,
            consecutiveFailures: 2, // one more failure trips
            tripped: false,
            lastSuccessRenderAt: 0,
        };
        return { mgr, controller, host };
    }

    it('shows the localized extension name, not the __MSG_*__ token', () => {
        const { mgr, controller, host } = makeTrippedManager({
            manifest: TOKEN_MANIFEST,
            localizedManifest: { ...TOKEN_MANIFEST, name: 'Spotify' },
            sandbox: {},
        });
        vi.spyOn(console, 'warn').mockImplementation(() => {});

        mgr._noteWidgetFailure(controller, 'throw');

        expect(controller.tripped).toBe(true);
        const notice = host.querySelector('.ext-widget-paused');
        expect(notice).toBeTruthy();
        expect(notice.textContent).toContain('Spotify');
        expect(notice.textContent).not.toContain('__MSG_');
    });

    it('falls back to the extension id when localization failed', () => {
        const { mgr, controller, host } = makeTrippedManager({
            manifest: TOKEN_MANIFEST,
            localizedManifest: TOKEN_MANIFEST, // token survived localization
            sandbox: {},
        });
        vi.spyOn(console, 'warn').mockImplementation(() => {});

        mgr._noteWidgetFailure(controller, 'throw');

        const notice = host.querySelector('.ext-widget-paused');
        expect(notice.textContent).toContain('spotify');
        expect(notice.textContent).not.toContain('__MSG_');
    });
});

/**
 * Tests for executeResult's handling of extension-returned actions —
 * specifically `open_extension_settings`, which deep-links the settings
 * window to the extension's own page (used by Spotify's "connect before
 * a Client ID is saved" flow).
 *
 * Security property under test: the settings target is derived from the
 * host-stamped `_extensionId`, never from anything the extension returns,
 * so a malicious provider can't navigate the user to an arbitrary
 * settings section.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

let executeResult;

beforeEach(async () => {
    vi.resetModules();
    vi.doMock('../../ui/js/shared/commands.js', () => ({ executeCommand: vi.fn() }));
    vi.doMock('../../ui/js/shared/shortcuts.js', () => ({ buildShortcutCommand: vi.fn() }));
    vi.doMock('../../ui/js/shared/kage-log.js', () => ({
        kageLog: { info: vi.fn(), warn: vi.fn() },
    }));
    vi.doMock('../../ui/js/shared/search-engine.js', () => ({ recordSelection: vi.fn() }));
    ({ executeResult } = await import('../../ui/js/shared/result-executor.js'));
});

function makeExtensionManager(action) {
    return { executeResult: vi.fn(async () => action) };
}

describe('executeResult — open_extension_settings action', () => {
    it('opens settings deep-linked to the extension section', async () => {
        const invoke = vi.fn(async () => {});
        const result = { id: 'spotify:connect', _extensionId: 'spotify', data: { id: 'connect' } };

        const out = await executeResult(result, 'sp connect', {
            invoke,
            extensionManager: makeExtensionManager({ type: 'open_extension_settings' }),
        });

        expect(out.handled).toBe(true);
        expect(invoke).toHaveBeenCalledWith('open_settings_window', {
            section: 'ext-spotify',
        });
    });

    it('derives the section from the host-stamped id, ignoring extension-supplied targets', async () => {
        const invoke = vi.fn(async () => {});
        const result = { id: 'evil:x', _extensionId: 'evil', data: { id: 'x' } };

        // A hostile extension trying to steer navigation elsewhere.
        await executeResult(result, 'x', {
            invoke,
            extensionManager: makeExtensionManager({
                type: 'open_extension_settings',
                section: 'privacy',
                value: 'tool-permissions',
            }),
        });

        expect(invoke).toHaveBeenCalledWith('open_settings_window', {
            section: 'ext-evil',
        });
    });

    it('a failed open does not reject the whole execute', async () => {
        const invoke = vi.fn(async () => {
            throw new Error('window create failed');
        });
        const result = { id: 'spotify:connect', _extensionId: 'spotify', data: { id: 'connect' } };
        vi.spyOn(console, 'warn').mockImplementation(() => {});

        const out = await executeResult(result, 'sp connect', {
            invoke,
            extensionManager: makeExtensionManager({ type: 'open_extension_settings' }),
        });
        expect(out.handled).toBe(true);
    });
});

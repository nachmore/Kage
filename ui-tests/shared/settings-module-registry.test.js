/**
 * Tests for ui/js/settings/module-registry.js — the small singleton
 * that lets settings modules look up their `SettingsManager` peer
 * without reaching into a window global.
 *
 * The registry is intentionally tiny but it's load-order sensitive:
 *   - if it returns null before main.js calls setSettingsManager, the
 *     dependent modules need to handle that gracefully (they do via
 *     `?.modules?.find(...)`)
 *   - re-importing the module fresh between tests must reset the
 *     handle (otherwise tests leak state across each other)
 *
 * Lock both behaviours in.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

let getSettingsManager;
let setSettingsManager;
let registerSettingsActions;

beforeEach(async () => {
    vi.resetModules();
    document.body.innerHTML = '';
    const mod = await import('../../ui/js/settings/module-registry.js');
    getSettingsManager = mod.getSettingsManager;
    setSettingsManager = mod.setSettingsManager;
    registerSettingsActions = mod.registerSettingsActions;
});

describe('manager singleton', () => {
    it('returns null until the manager is set', () => {
        expect(getSettingsManager()).toBeNull();
    });

    it('hands back whatever was passed to setSettingsManager', () => {
        const mgr = { modules: [] };
        setSettingsManager(mgr);
        expect(getSettingsManager()).toBe(mgr);
    });

    it('lets callers replace the manager in-place (e.g. window reuse)', () => {
        const a = { id: 'a' };
        const b = { id: 'b' };
        setSettingsManager(a);
        setSettingsManager(b);
        expect(getSettingsManager()).toBe(b);
    });
});

describe('action dispatcher re-export', () => {
    it('re-exports registerSettingsActions so modules can import once', () => {
        // Sanity: the re-export must be a function that takes a map of
        // handlers. We don't actually fire a click here because the
        // settings-actions test already exercises the click → handler
        // path; this test just locks in that the re-export is wired
        // (a typo in the `export { ... } from './actions.js'` line
        // would still satisfy a presence-only assertion, but a typo
        // in the `import` would reject at module-load time anyway).
        expect(typeof registerSettingsActions).toBe('function');
        expect(() => registerSettingsActions({ 'noop': () => {} })).not.toThrow();
    });
});

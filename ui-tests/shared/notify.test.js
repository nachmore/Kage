/**
 * Tests for sendAppNotification's Windows-clickable-first / plugin-fallback
 * dispatch.
 *
 * Contract:
 *   - honours the show_notifications=false opt-out (no toast at all);
 *   - tries the Rust `notify_response_ready` command first, passing the source
 *     window as targetLabel;
 *   - if that returns true (clickable toast shown), does NOT also fire the
 *     plugin toast (that was the double-notification failure mode);
 *   - if it returns false (dev/non-Windows) or throws, falls back to the
 *     plugin `sendNotification`.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';

let sendNotification;
let isPermissionGranted;
let requestPermission;

function installTauri() {
    sendNotification = vi.fn();
    isPermissionGranted = vi.fn(async () => true);
    requestPermission = vi.fn(async () => 'granted');
    globalThis.window = globalThis.window || globalThis;
    window.__TAURI__ = {
        event: { listen: vi.fn(async () => () => {}) },
        notification: { sendNotification, isPermissionGranted, requestPermission },
    };
}

async function loadModule() {
    return await import('../../ui/js/shared/notify.js');
}

// getConfig reads from config-cache, which invokes 'get_config'. Route that
// through the same invoke mock so we control show_notifications.
function makeInvoke({ showNotifications = true, notifyReturns = true, notifyThrows = false } = {}) {
    return vi.fn(async (cmd) => {
        if (cmd === 'get_config') {
            return { system: { show_notifications: showNotifications } };
        }
        if (cmd === 'notify_response_ready') {
            if (notifyThrows) throw new Error('command missing');
            return notifyReturns;
        }
        return undefined;
    });
}

describe('sendAppNotification', () => {
    beforeEach(() => {
        vi.resetModules();
        installTauri();
    });

    it('does nothing when show_notifications is disabled', async () => {
        const { sendAppNotification } = await loadModule();
        const invoke = makeInvoke({ showNotifications: false });
        await sendAppNotification(invoke, 'T', 'B', 'floating');
        expect(invoke).not.toHaveBeenCalledWith('notify_response_ready', expect.anything());
        expect(sendNotification).not.toHaveBeenCalled();
    });

    it('uses the clickable command and passes the source as targetLabel', async () => {
        const { sendAppNotification } = await loadModule();
        const invoke = makeInvoke({ notifyReturns: true });
        await sendAppNotification(invoke, 'Title', 'Body', 'floating');
        expect(invoke).toHaveBeenCalledWith('notify_response_ready', {
            title: 'Title',
            body: 'Body',
            targetLabel: 'floating',
        });
    });

    it('does NOT fire the plugin toast when the clickable toast was shown', async () => {
        const { sendAppNotification } = await loadModule();
        const invoke = makeInvoke({ notifyReturns: true });
        await sendAppNotification(invoke, 'T', 'B', 'main');
        expect(sendNotification).not.toHaveBeenCalled();
    });

    it('falls back to the plugin toast when the command returns false', async () => {
        const { sendAppNotification } = await loadModule();
        const invoke = makeInvoke({ notifyReturns: false });
        await sendAppNotification(invoke, 'T', 'B', 'main');
        expect(sendNotification).toHaveBeenCalledWith({ title: 'T', body: 'B' });
    });

    it('falls back to the plugin toast when the command throws', async () => {
        const { sendAppNotification } = await loadModule();
        const invoke = makeInvoke({ notifyThrows: true });
        await sendAppNotification(invoke, 'T', 'B', 'main');
        expect(sendNotification).toHaveBeenCalledWith({ title: 'T', body: 'B' });
    });

    it('defaults targetLabel to main when source is missing', async () => {
        const { sendAppNotification } = await loadModule();
        const invoke = makeInvoke({ notifyReturns: true });
        await sendAppNotification(invoke, 'T', 'B');
        expect(invoke).toHaveBeenCalledWith('notify_response_ready', {
            title: 'T',
            body: 'B',
            targetLabel: 'main',
        });
    });
});

/**
 * Permission Modal Handler — Floating Window
 *
 * Uses shared permissions-core for all common logic.
 * Adds floating-specific behavior:
 *   - Resizes the floating window to fit the modal
 *   - Checks session ownership (only handles floating session requests)
 *   - Sends OS notifications when window is hidden
 *   - Checks has_pending_permission before showing
 */

import { createPermissionHandler } from '../shared/permissions-core.js';

const { invoke } = window.__TAURI__.core;
const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();

const handler = createPermissionHandler(invoke, appWindow, {
    // Resize the floating window to fit the permission modal
    async onShow(modal) {
        // Pause mascot animations while modal is open
        if (window._kageMascot) window._kageMascot.pause();

        // Pause auto-resize while the modal sizes itself
        window._floatingApp?.windowManager?.suspendAutoResize();

        try {
            await appWindow.setFocus();
            await appWindow.setAlwaysOnTop(true);
        } catch (error) {
            console.error('Failed to set window focus:', error);
        }

        // Two-pass resize: first grow large enough, then measure and fine-tune
        try {
            const scale = window.devicePixelRatio || 1;

            await invoke('resize_floating_window', {
                width: Math.round(540 * scale),
                height: Math.round(700 * scale)
            });

            await new Promise(resolve => setTimeout(resolve, 50));

            const modalEl = modal.querySelector('.permission-modal');
            if (modalEl) {
                const rect = modalEl.getBoundingClientRect();
                const neededHeight = Math.round((rect.height + 80) * scale);
                const neededWidth = Math.round(540 * scale);
                const finalHeight = Math.max(neededHeight, Math.round(600 * scale));
                await invoke('resize_floating_window', {
                    width: neededWidth,
                    height: finalHeight
                });
            }
        } catch (error) {
            console.error('Failed to resize window for modal:', error);
        }
    },

    // Resize back to fit content after modal is hidden
    async onHide(_modal, hasQueuedNext) {
        // Resume mascot animations (only when no more queued modals)
        if (!hasQueuedNext && window._kageMascot) window._kageMascot.resume();

        if (hasQueuedNext) return; // Next modal will handle sizing
        try {
            window._floatingApp?.windowManager?.resumeAutoResize();
        } catch (e) { /* ignore */ }
    },

    // Only handle requests for the floating window's own session
    async onRequestReceived(event, invoke, appWindow) {
        const { notification, auto_approve } = event.payload;
        const requestSessionId = notification.params?.sessionId || '';

        let floatingSessionId = null;
        try {
            floatingSessionId = await invoke('get_floating_session_id');
        } catch (e) { console.warn('[Permissions] Failed to get floating session ID:', e); }

        const source = event.payload.source || '';
        const isFloatingSession = requestSessionId && floatingSessionId
            && requestSessionId === floatingSessionId;

        let isVisible = false;
        try { isVisible = await appWindow.isVisible(); } catch {}

        if (source !== 'floating' && !isVisible) {
            console.log('Ignoring permission request — originated from chat, floating hidden');
            return { handle: false };
        }

        if (!isFloatingSession) {
            console.log('Ignoring permission request — not for floating session');
            return { handle: false };
        }

        if (auto_approve) {
            return { handle: true };
        }

        // Double-check the request is still pending
        let stillPending = false;
        try {
            stillPending = await invoke('has_pending_permission');
        } catch (e) { stillPending = true; }

        if (!stillPending) {
            console.log('Permission request already handled, skipping modal');
            return { handle: false };
        }

        const currentlyVisible = await appWindow.isVisible();
        const eventSource = event.payload.source || 'floating';

        // Force-show the floating window if the request originated from it
        if (!currentlyVisible && eventSource === 'floating') {
            const toolTitle = notification.params?.toolCall?.title || 'Unknown Tool';
            const toolName = event.payload.toolName || '';
            const body = toolName ? `${toolName}: ${toolTitle}` : toolTitle;
            try {
                const notif = window.__TAURI__?.notification;
                if (notif) {
                    let granted = await notif.isPermissionGranted();
                    if (!granted) {
                        const perm = await notif.requestPermission();
                        granted = perm === 'granted';
                    }
                    if (granted) {
                        notif.sendNotification({
                            title: '🔐 Tool Permission Required',
                            body: body
                        });
                    }
                }
            } catch { /* ignore */ }
            await appWindow.show();
            await appWindow.setFocus();
        }

        const nowVisible = await appWindow.isVisible();
        if (!nowVisible) return { handle: false };

        // Force any pending throttled streaming render to paint NOW so the
        // user sees the complete streamed text behind the dialog rather
        // than whatever partial state the debounce timer happened to be in.
        // The MessageStreamController used to do this on every tool_call
        // update — wasteful, since most tool events don't open a modal.
        // Now it only fires on the path that actually surfaces a dialog.
        window._floatingApp?.flushStreamingRender();

        return { handle: true };
    }
});

// Initialize
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => handler.init());
} else {
    handler.init();
}

// Export for use by FloatingApp (extension tool permission checks)
window.PermissionModal = {
    show: handler.show,
    hide: handler.hide,
    showForExtensionTool: handler.showForExtensionTool,
};

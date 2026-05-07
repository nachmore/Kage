/**
 * Permission Modal Handler — Chat Window
 *
 * Uses shared permissions-core for all common logic.
 * Adds chat-specific behavior:
 *   - Session-scoped modal (hides when switching sessions)
 *   - Exposes onSessionSwitch / hasPendingRequest for ChatApp
 */

import { createPermissionHandler } from '../shared/permissions-core.js';
import { waitForTauri } from '../shared/tauri-init.js';

waitForTauri(({ invoke, appWindow }) => {

    const handler = createPermissionHandler(invoke, appWindow, {
        // Store session ID on the modal element for session-scoping
        onShow(modal) {
            const req = handler.getCurrentRequest();
            if (req) modal.dataset.sessionId = req.sessionId;
        },
        onHide(modal) {
            if (modal) modal.dataset.sessionId = '';
        },
        // Chat window accepts all permission requests without filtering
        // (no session ownership check like floating does). The hook is
        // wired purely to trigger a streaming-render flush before the
        // dialog opens — without this, the user might see a half-rendered
        // chunk of the agent's response sitting behind the modal because
        // the throttled streaming path was mid-debounce when the request
        // arrived.
        async onRequestReceived(event) {
            const { auto_approve } = event.payload;
            if (!auto_approve) {
                window._chatApp?.flushStreamingRender();
            }
            return { handle: true };
        },
    });

    handler.init();

    // Expose functions for ChatApp to call when switching sessions
    window.ChatPermissions = {
        /** Hide the modal if the active session doesn't match */
        onSessionSwitch(newSessionId) {
            const modal = document.getElementById('permissionModal');
            const req = handler.getCurrentRequest();
            if (!modal || !req) return;
            if (req.sessionId !== newSessionId) {
                // Different session — hide but don't dismiss (keep the request pending)
                modal.style.display = 'none';
            } else {
                // Same session — show it again
                modal.style.display = 'flex';
            }
        },
        /** Check if there's a pending request for a given session */
        hasPendingRequest(sessionId) {
            const req = handler.getCurrentRequest();
            return req && req.sessionId === sessionId;
        },
        /** Show the permission modal for an extension tool call. Returns promise<boolean>. */
        showForExtensionTool: handler.showForExtensionTool,
    };
});

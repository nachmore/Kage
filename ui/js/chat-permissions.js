/**
 * Permission Modal Handler for expanded chat window.
 * - Scoped to the chat content area (not the whole window)
 * - Tracks which session the request belongs to
 * - Hides when switching to a different session
 */

function waitForTauri(callback) {
    if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.webviewWindow) {
        callback();
    } else {
        setTimeout(() => waitForTauri(callback), 50);
    }
}

waitForTauri(() => {
    const { invoke } = window.__TAURI__.core;
    const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();

    let currentPermissionRequest = null;

    function showPermissionModal(notification) {
        const modal = document.getElementById('permissionModal');
        const toolTitle = document.getElementById('permissionToolTitle');
        if (!modal || !toolTitle) return;

        const params = notification.params || {};
        const toolCall = params.toolCall || {};
        const sessionId = params.sessionId || '';

        currentPermissionRequest = {
            id: notification.id,
            sessionId: sessionId,
            toolCall: toolCall,
            options: params.options || []
        };

        // Store the session this request belongs to
        modal.dataset.sessionId = sessionId;

        toolTitle.textContent = toolCall.title || 'Unknown Tool';
        modal.style.display = 'flex';
    }

    function hidePermissionModal() {
        const modal = document.getElementById('permissionModal');
        if (modal) modal.style.display = 'none';
        currentPermissionRequest = null;
        modal.dataset.sessionId = '';
    }

    async function handlePermissionResponse(optionId, policyOverride) {
        if (!currentPermissionRequest) return;

        try {
            await invoke('send_permission_response', {
                requestId: currentPermissionRequest.id,
                optionId: optionId,
                toolTitle: currentPermissionRequest.toolCall.title || 'Unknown'
            });

            if (policyOverride) {
                await invoke('update_tool_policy', {
                    toolTitle: currentPermissionRequest.toolCall.title || 'Unknown',
                    policy: policyOverride
                });
            }

            await new Promise(r => setTimeout(r, 100));
            hidePermissionModal();
        } catch (error) {
            console.error('Failed to send permission response:', error);
        }
    }

    // Button handlers
    document.getElementById('permissionDenyAlways')?.addEventListener('click', () => handlePermissionResponse('reject_once', 'deny'));
    document.getElementById('permissionDeny')?.addEventListener('click', () => handlePermissionResponse('reject_once'));
    document.getElementById('permissionOnce')?.addEventListener('click', () => handlePermissionResponse('allow_once'));
    document.getElementById('permissionAlways')?.addEventListener('click', () => handlePermissionResponse('allow_always'));

    // Listen for permission requests
    appWindow.listen('permission_request', (event) => {
        const { notification, auto_approve } = event.payload;
        if (auto_approve) {
            invoke('send_permission_response', {
                requestId: notification.id,
                optionId: 'allow_once',
                toolTitle: notification.params?.toolCall?.title || 'Unknown'
            }).catch(e => console.error('Auto-approve failed:', e));
        } else {
            showPermissionModal(notification);
        }
    });

    // Close on overlay click
    document.getElementById('permissionModal')?.addEventListener('click', (e) => {
        if (e.target.id === 'permissionModal') {
            handlePermissionResponse('reject_once');
        }
    });

    // Listen for external dismissal (e.g. floating window auto-denied the request)
    appWindow.listen('permission_dismissed', () => {
        console.log('Permission dismissed externally');
        hidePermissionModal();
    });

    // Expose functions for ChatApp to call when switching sessions
    window.ChatPermissions = {
        /** Hide the modal if the active session doesn't match */
        onSessionSwitch(newSessionId) {
            const modal = document.getElementById('permissionModal');
            if (!modal || !currentPermissionRequest) return;
            if (currentPermissionRequest.sessionId !== newSessionId) {
                // Different session — hide but don't dismiss (keep the request pending)
                modal.style.display = 'none';
            } else {
                // Same session — show it again
                modal.style.display = 'flex';
            }
        },
        /** Check if there's a pending request for a given session */
        hasPendingRequest(sessionId) {
            return currentPermissionRequest && currentPermissionRequest.sessionId === sessionId;
        }
    };
});

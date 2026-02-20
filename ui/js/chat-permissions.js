/**
 * Permission Modal Handler for expanded chat window.
 * Reuses the same logic as floating-permissions.js.
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

        currentPermissionRequest = {
            id: notification.id,
            sessionId: params.sessionId,
            toolCall: toolCall,
            options: params.options || []
        };

        toolTitle.textContent = toolCall.title || 'Unknown Tool';
        modal.style.display = 'flex';
    }

    function hidePermissionModal() {
        const modal = document.getElementById('permissionModal');
        if (modal) modal.style.display = 'none';
        currentPermissionRequest = null;
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
});

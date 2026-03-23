/**
 * Permission Modal Handler for expanded chat window.
 * - Scoped to the chat content area (not the whole window)
 * - Tracks which session the request belongs to
 * - Hides when switching to a different session
 */

import { getToolEmoji, escapeHtml } from '../shared/tool-utils.js';

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
    let _extensionToolCallback = null;
    let _permissionQueue = [];

    function showPermissionModal(notification, toolName) {
        const modal = document.getElementById('permissionModal');
        const toolTitle = document.getElementById('permissionToolTitle');
        const toolNameEl = document.getElementById('permissionToolName');
        if (!modal || !toolTitle) return;

        // If a permission is already showing, queue this one
        if (currentPermissionRequest && modal.style.display === 'flex') {
            _permissionQueue.push({ notification, toolName });
            console.log(`[Permissions] Queued permission request (${_permissionQueue.length} in queue)`);
            return;
        }

        const params = notification.params || {};
        const toolCall = params.toolCall || {};
        const sessionId = params.sessionId || '';

        currentPermissionRequest = {
            id: notification.id,
            sessionId: sessionId,
            toolCall: toolCall,
            options: params.options || [],
            toolName: toolName || null
        };

        // Store the session this request belongs to
        modal.dataset.sessionId = sessionId;

        toolTitle.textContent = toolCall.title || 'Unknown Tool';

        // Show tool name with emoji if available
        if (toolNameEl) {
            if (toolName) {
                const emoji = getToolEmoji(toolName);
                toolNameEl.innerHTML = `<span class="tool-emoji">${emoji}</span><span class="tool-label">${escapeHtml(toolName)}</span>`;
                toolNameEl.style.display = 'flex';
            } else {
                toolNameEl.style.display = 'none';
            }
        }

        modal.style.display = 'flex';
    }

    function hidePermissionModal() {
        const modal = document.getElementById('permissionModal');
        if (modal) modal.style.display = 'none';
        currentPermissionRequest = null;
        if (modal) modal.dataset.sessionId = '';

        // Show next queued permission request
        if (_permissionQueue.length > 0) {
            const next = _permissionQueue.shift();
            console.log(`[Permissions] Showing next queued request (${_permissionQueue.length} remaining)`);
            setTimeout(() => showPermissionModal(next.notification, next.toolName), 150);
        }
    }

    async function handlePermissionResponse(optionId, policyOverride) {
        if (!currentPermissionRequest) return;

        try {
            const policyTitle = currentPermissionRequest.toolName || currentPermissionRequest.toolCall.title || 'Unknown';

            // Extension tool requests use a callback instead of ACP response
            if (_extensionToolCallback) {
                const allowed = optionId === 'allow_once' || optionId === 'allow_always';
                const updatePolicy = policyOverride || (optionId === 'allow_always' ? 'allow' : null);
                if (updatePolicy) {
                    await invoke('update_tool_policy', { toolTitle: policyTitle, policy: updatePolicy });
                }
                const cb = _extensionToolCallback;
                _extensionToolCallback = null;
                hidePermissionModal();
                cb(allowed);
                return;
            }

            await invoke('send_permission_response', {
                requestId: currentPermissionRequest.id,
                optionId: optionId,
                toolTitle: policyTitle
            });

            if (policyOverride) {
                await invoke('update_tool_policy', {
                    toolTitle: policyTitle,
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
            showPermissionModal(notification, event.payload.toolName);
        }
    });

    // Close on overlay click
    document.getElementById('permissionModal')?.addEventListener('click', (e) => {
        if (e.target.id === 'permissionModal') {
            handlePermissionResponse('reject_once');
        }
    });

    // Esc to deny, block all keyboard input while modal is open
    document.addEventListener('keydown', (e) => {
        if (!currentPermissionRequest) return;
        if (e.key === 'Escape') {
            e.preventDefault();
            e.stopPropagation();
            handlePermissionResponse('reject_once');
        } else {
            e.preventDefault();
            e.stopPropagation();
        }
    }, true);

    // Listen for external dismissal (e.g. floating window auto-denied the request)
    appWindow.listen('permission_dismissed', () => {
        console.log('Permission dismissed externally');
        _permissionQueue = []; // Clear queue — other window handled it
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
        },
        /**
         * Show the permission modal for an extension tool call.
         * Returns a promise that resolves to true (allowed) or false (denied).
         */
        showForExtensionTool(extensionId, toolName, icon) {
            return new Promise((resolve) => {
                const toolTitle = `ext:${extensionId}/${toolName}`;
                const notification = {
                    id: null,
                    params: {
                        toolCall: {
                            title: `${icon} ${extensionId}/${toolName}`,
                        },
                        options: [],
                    },
                };
                _extensionToolCallback = resolve;
                showPermissionModal(notification, toolTitle);
            });
        }
    };
});

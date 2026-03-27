/**
 * Shared Permission Modal Core
 *
 * Contains all the common logic for showing/hiding the permission modal,
 * handling responses, queuing, extension tool support, keyboard blocking,
 * and dismissal listeners.
 *
 * Each window (floating, chat) imports this and provides hooks for
 * window-specific behavior via the `hooks` parameter.
 *
 * Hooks:
 *   onShow(modal, notification, toolName)  — called after modal is displayed (e.g. resize window)
 *   onHide(modal, hasQueuedNext)           — called after modal is hidden (e.g. resize back)
 *   onRequestReceived(event, invoke, appWindow) — called when permission_request arrives;
 *       return { handle: true/false, notification, toolName, autoApprove } to control behavior.
 *       If handle is false, the request is ignored.
 */

import { getToolEmoji, escapeHtml } from './tool-utils.js';

export function createPermissionHandler(invoke, appWindow, hooks = {}) {
    let currentPermissionRequest = null;
    let _extensionToolCallback = null;
    let _permissionQueue = [];

    async function showPermissionModal(notification, toolName) {
        const modal = document.getElementById('permissionModal');
        const toolTitleEl = document.getElementById('permissionToolTitle');
        const toolNameEl = document.getElementById('permissionToolName');
        if (!modal || !toolTitleEl) return;

        // If a permission is already showing, queue this one
        if (currentPermissionRequest && modal.style.display === 'flex') {
            _permissionQueue.push({ notification, toolName });
            console.log(`[Permissions] Queued permission request (${_permissionQueue.length} in queue)`);
            return;
        }

        const params = notification.params || {};
        const toolCall = params.toolCall || {};

        currentPermissionRequest = {
            id: notification.id,
            sessionId: params.sessionId || '',
            toolCall: toolCall,
            options: params.options || [],
            toolName: toolName || null
        };

        toolTitleEl.textContent = toolCall.title || 'Unknown Tool';

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

        // Window-specific show behavior (e.g. resize, focus) — must await for sizing
        if (hooks.onShow) await hooks.onShow(modal, notification, toolName);
    }

    async function hidePermissionModal() {
        const modal = document.getElementById('permissionModal');
        if (modal) modal.style.display = 'none';
        currentPermissionRequest = null;

        // Show next queued permission request
        if (_permissionQueue.length > 0) {
            const next = _permissionQueue.shift();
            console.log(`[Permissions] Showing next queued request (${_permissionQueue.length} remaining)`);
            setTimeout(async () => await showPermissionModal(next.notification, next.toolName), 150);
            if (hooks.onHide) await hooks.onHide(modal, true);
            return;
        }

        // Window-specific hide behavior (e.g. resize back)
        if (hooks.onHide) await hooks.onHide(modal, false);
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
                await hidePermissionModal();
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

            // Small delay to ensure the response is processed
            await new Promise(r => setTimeout(r, 100));
            await hidePermissionModal();
        } catch (error) {
            console.error('Failed to send permission response:', error);
        }
    }

    function wireButtons() {
        document.getElementById('permissionDenyAlways')?.addEventListener('click', () => handlePermissionResponse('reject_once', 'deny'));
        document.getElementById('permissionDeny')?.addEventListener('click', () => handlePermissionResponse('reject_once'));
        document.getElementById('permissionOnce')?.addEventListener('click', () => handlePermissionResponse('allow_once'));
        document.getElementById('permissionAlways')?.addEventListener('click', () => handlePermissionResponse('allow_always'));
    }

    function wireOverlayDismiss() {
        document.getElementById('permissionModal')?.addEventListener('click', (e) => {
            if (e.target.id === 'permissionModal' || e.target === document.getElementById('permissionModal')) {
                handlePermissionResponse('reject_once');
            }
        });
    }

    function wireKeyboard() {
        document.addEventListener('keydown', (e) => {
            if (!currentPermissionRequest) return;
            if (e.key === 'Escape') {
                e.preventDefault();
                e.stopPropagation();
                handlePermissionResponse('reject_once');
            } else {
                // Block typing from reaching the input behind the modal
                e.preventDefault();
                e.stopPropagation();
            }
        }, true);
    }

    function wirePermissionRequestListener() {
        appWindow.listen('permission_request', async (event) => {
            const { notification, auto_approve } = event.payload;

            // Let the window-specific hook decide whether to handle this request
            if (hooks.onRequestReceived) {
                const decision = await hooks.onRequestReceived(event, invoke, appWindow);
                if (!decision || !decision.handle) return;
            }

            if (auto_approve) {
                invoke('send_permission_response', {
                    requestId: notification.id,
                    optionId: 'allow_once',
                    toolTitle: notification.params?.toolCall?.title || 'Unknown'
                }).catch(e => console.error('Auto-approve failed:', e));
            } else {
                await showPermissionModal(notification, event.payload.toolName);
            }
        });
    }

    function wireDismissalListener() {
        appWindow.listen('permission_dismissed', () => {
            console.log('Permission dismissed externally');
            _permissionQueue = [];
            hidePermissionModal();
        });
    }

    /** Standard init: wire buttons, overlay, keyboard, listeners */
    function init() {
        wireButtons();
        wireOverlayDismiss();
        wireKeyboard();
        wirePermissionRequestListener();
        wireDismissalListener();
    }

    /** Show the permission modal for an extension tool call. Returns promise<boolean>. */
    function showForExtensionTool(extensionId, toolName, icon) {
        return new Promise(async (resolve) => {
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
            await showPermissionModal(notification, toolTitle);
        });
    }

    return {
        init,
        show: showPermissionModal,
        hide: hidePermissionModal,
        showForExtensionTool,
        /** Get the current permission request (for session-scoping in chat) */
        getCurrentRequest() { return currentPermissionRequest; },
    };
}

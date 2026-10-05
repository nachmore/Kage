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
import { EVT } from './events.js';
import { t } from './i18n.js';

export function createPermissionHandler(invoke, appWindow, hooks = {}) {
    let currentPermissionRequest = null;
    let _permissionQueue = [];

    // Extension-tool prompts carry their resolve fn on the notification
    // (`extCallback`) so it travels through the queue with its own request
    // — a module-level callback would capture the answer to whichever
    // prompt happens to be visible. Any path that drops a request without
    // an answer must resolve it false so the awaiting tool call ends.
    function rejectDropped(req) {
        const cb = req?.extCallback;
        if (!cb) return;
        req.extCallback = null;
        cb(false);
    }

    function isShowing(modal) {
        return modal?.style.display === 'flex';
    }

    async function showPermissionModal(notification, toolName) {
        const modal = document.getElementById('permissionModal');
        const toolTitleEl = document.getElementById('permissionToolTitle');
        const toolNameEl = document.getElementById('permissionToolName');
        if (!modal || !toolTitleEl) {
            rejectDropped(notification);
            return;
        }

        // If a permission is showing, queue this one.
        if (currentPermissionRequest && isShowing(modal)) {
            _permissionQueue.push({ notification, toolName });
            console.log(
                `[Permissions] Queued permission request (${_permissionQueue.length} in queue)`
            );
            return;
        }

        // Pending but hidden (chat's onSessionSwitch hides another session's
        // request): show the new one, and put the hidden one back at the
        // front of the queue rather than overwriting — and orphaning — it.
        // A request mid-answer is already on its way out; don't requeue it.
        const hidden = currentPermissionRequest;
        if (hidden && !hidden.answering) {
            _permissionQueue.unshift({
                notification: { ...hidden.notification, extCallback: hidden.extCallback },
                toolName: hidden.toolName,
            });
            console.log('[Permissions] Requeued a hidden request behind a new one');
        }

        const params = notification.params || {};
        const toolCall = params.toolCall || {};

        currentPermissionRequest = {
            id: notification.id,
            sessionId: params.sessionId || '',
            toolCall: toolCall,
            options: params.options || [],
            toolName: toolName || null,
            isExtension: !!notification.extCallback,
            extCallback: notification.extCallback || null,
            notification,
        };

        toolTitleEl.textContent = toolCall.title || t('shared.permission.unknown_tool');

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
        // Hidden without an answer (external dismissal) — release the waiter.
        rejectDropped(currentPermissionRequest);
        currentPermissionRequest = null;

        // Show next queued permission request
        if (_permissionQueue.length > 0) {
            const next = _permissionQueue.shift();
            console.log(
                `[Permissions] Showing next queued request (${_permissionQueue.length} remaining)`
            );
            setTimeout(
                async () => await showPermissionModal(next.notification, next.toolName),
                150
            );
            if (hooks.onHide) await hooks.onHide(modal, true);
            return;
        }

        // Window-specific hide behavior (e.g. resize back)
        if (hooks.onHide) await hooks.onHide(modal, false);
    }

    async function handlePermissionResponse(optionId, policyOverride, grantType) {
        if (!currentPermissionRequest) return;

        // Close dropdowns if open
        const allowDropdown = document.getElementById('permissionAllowDropdown');
        const denyDropdown = document.getElementById('permissionDenyDropdown');
        if (allowDropdown) allowDropdown.style.display = 'none';
        if (denyDropdown) denyDropdown.style.display = 'none';

        // Requests can advance while we await IPC below (external dismissal
        // shows the next queued one); only hide if ours is still current,
        // or we'd hide — and for extensions, auto-deny — the next request.
        const req = currentPermissionRequest;
        const hideIfStillCurrent = async () => {
            if (currentPermissionRequest === req) await hidePermissionModal();
        };
        // An answer is in flight: a new request arriving now (after a session
        // switch hid this one) must not requeue it.
        req.answering = true;

        try {
            const policyTitle =
                currentPermissionRequest.toolName ||
                currentPermissionRequest.toolCall.title ||
                t('shared.permission.unknown');

            // Extension tool requests use a callback instead of ACP response
            if (currentPermissionRequest.isExtension) {
                const allowed = optionId === 'allow_once' || optionId === 'allow_always';
                const cb = currentPermissionRequest.extCallback;
                // Already answered (double-click while the policy update
                // was in flight).
                if (!cb) return;
                // Claim the callback first so hidePermissionModal doesn't
                // treat this answered request as dropped.
                currentPermissionRequest.extCallback = null;
                if (policyOverride) {
                    await invoke('update_tool_policy', {
                        toolTitle: policyTitle,
                        policy: policyOverride,
                        grantType: grantType || 'once',
                    }).catch((e) => console.error('Failed to update tool policy:', e));
                }
                await hideIfStillCurrent();
                cb(allowed);
                return;
            }

            await invoke('send_permission_response', {
                sessionId: currentPermissionRequest.sessionId || null,
                requestId: currentPermissionRequest.id,
                optionId: optionId,
                toolTitle: policyTitle,
            });

            if (policyOverride) {
                await invoke('update_tool_policy', {
                    toolTitle: policyTitle,
                    policy: policyOverride,
                    grantType: grantType || 'once',
                });
            }

            // Small delay to ensure the response is processed
            await new Promise((r) => setTimeout(r, 100));
            await hideIfStillCurrent();
        } catch (error) {
            // Still unanswered — the user can retry.
            req.answering = false;
            console.error('Failed to send permission response:', error);
        }
    }

    function wireButtons() {
        // Deny button (single use)
        document
            .getElementById('permissionDeny')
            ?.addEventListener('click', () => handlePermissionResponse('reject_once'));

        // Deny dropdown toggle
        const denyMenuBtn = document.getElementById('permissionDenyMenu');
        const denyDropdown = document.getElementById('permissionDenyDropdown');
        if (denyMenuBtn && denyDropdown) {
            denyMenuBtn.addEventListener('click', (e) => {
                e.stopPropagation();
                denyDropdown.style.display =
                    denyDropdown.style.display === 'none' ? 'block' : 'none';
                // Close the other dropdown
                const allowDropdown = document.getElementById('permissionAllowDropdown');
                if (allowDropdown) allowDropdown.style.display = 'none';
            });
        }

        // Deny always
        document
            .getElementById('permissionDenyAlways')
            ?.addEventListener('click', () => handlePermissionResponse('reject_once', 'deny'));

        // Allow button (single use)
        document
            .getElementById('permissionAllow')
            ?.addEventListener('click', () =>
                handlePermissionResponse('allow_once', 'allow', 'once')
            );

        // Allow dropdown toggle
        const menuBtn = document.getElementById('permissionAllowMenu');
        const dropdown = document.getElementById('permissionAllowDropdown');
        if (menuBtn && dropdown) {
            menuBtn.addEventListener('click', (e) => {
                e.stopPropagation();
                dropdown.style.display = dropdown.style.display === 'none' ? 'block' : 'none';
                // Close the other dropdown
                if (denyDropdown) denyDropdown.style.display = 'none';
            });
        }

        // Close all dropdowns when clicking outside
        document.addEventListener('click', () => {
            if (denyDropdown) denyDropdown.style.display = 'none';
            if (dropdown) dropdown.style.display = 'none';
        });

        // Allow dropdown items
        document
            .getElementById('permissionAllow24h')
            ?.addEventListener('click', () =>
                handlePermissionResponse('allow_once', 'allow', '24h')
            );
        document
            .getElementById('permissionAllowAlways')
            ?.addEventListener('click', () =>
                handlePermissionResponse('allow_once', 'allow', 'always')
            );
    }

    function wireOverlayDismiss() {
        document.getElementById('permissionModal')?.addEventListener('click', (e) => {
            if (
                e.target.id === 'permissionModal' ||
                e.target === document.getElementById('permissionModal')
            ) {
                handlePermissionResponse('reject_once');
            }
        });
    }

    function wireKeyboard() {
        document.addEventListener(
            'keydown',
            (e) => {
                // A request hidden by a session switch must not swallow
                // typing in the session the user is actually in.
                if (!currentPermissionRequest) return;
                if (!isShowing(document.getElementById('permissionModal'))) return;
                if (e.key === 'Escape') {
                    e.preventDefault();
                    e.stopPropagation();
                    handlePermissionResponse('reject_once');
                } else {
                    // Block typing from reaching the input behind the modal
                    e.preventDefault();
                    e.stopPropagation();
                }
            },
            true
        );
    }

    function wirePermissionRequestListener() {
        appWindow.listen('permission_request', async (event) => {
            const { notification, auto_approve } = event.payload;

            // Let the window-specific hook decide whether to handle this request
            if (hooks.onRequestReceived) {
                const decision = await hooks.onRequestReceived(event, invoke, appWindow);
                if (!decision?.handle) return;
            }

            if (auto_approve) {
                invoke('send_permission_response', {
                    sessionId: notification.params?.sessionId || null,
                    requestId: notification.id,
                    optionId: 'allow_once',
                    toolTitle:
                        notification.params?.toolCall?.title || t('shared.permission.unknown'),
                }).catch((e) => console.error('Auto-approve failed:', e));
            } else {
                await showPermissionModal(notification, event.payload.toolName);
            }
        });
    }

    function wireDismissalListener() {
        appWindow.listen(EVT.PERMISSION_DISMISSED, (event) => {
            // The payload carries the dismissed request's id (multiple
            // windows can each have their own pending permission).
            const dismissedId = event?.payload?.requestId;
            if (dismissedId === undefined || dismissedId === null) {
                // Broadcast dismissal (legacy shape) — drop everything.
                console.log('Permission dismissed externally (broadcast)');
                for (const q of _permissionQueue) rejectDropped(q.notification);
                _permissionQueue = [];
                hidePermissionModal();
                return;
            }
            const matches = (id) => JSON.stringify(id) === JSON.stringify(dismissedId);
            // Remove any queued copy of that request either way.
            _permissionQueue = _permissionQueue.filter((q) => !matches(q.notification?.id));
            if (currentPermissionRequest && matches(currentPermissionRequest.id)) {
                console.log('Permission dismissed externally');
                // hidePermissionModal advances to the next queued request.
                hidePermissionModal();
            }
        });
    }

    // Extension-tool prompts carry no ACP session id, so once a session
    // switch hid one, no later switch could show it again and its tool call
    // would wait forever. Keep them visible: put the modal back whenever
    // something other than hidePermissionModal hides it (that one clears
    // currentPermissionRequest synchronously, before this callback runs).
    function wireExtensionPromptVisibility() {
        const modal = document.getElementById('permissionModal');
        if (!modal || typeof MutationObserver === 'undefined') return;
        new MutationObserver(() => {
            if (currentPermissionRequest?.isExtension && !isShowing(modal)) {
                modal.style.display = 'flex';
            }
        }).observe(modal, { attributes: true, attributeFilter: ['style'] });
    }

    /** Standard init: wire buttons, overlay, keyboard, listeners */
    function init() {
        wireButtons();
        wireOverlayDismiss();
        wireKeyboard();
        wireExtensionPromptVisibility();
        wirePermissionRequestListener();
        wireDismissalListener();
    }

    /** Show the permission modal for an extension tool call. Returns promise<boolean>. */
    function showForExtensionTool(extensionId, toolName, icon) {
        // The promise resolves when the user accepts/denies via the
        // modal — the resolve fn rides on the notification (`extCallback`)
        // and is invoked by the modal's button handlers. The modal show
        // itself is async (loads i18n, etc) but we don't need its
        // outcome here, so fire-and-forget the await rather than
        // wrapping the executor in `async` (which Biome flags because
        // it swallows rejections silently).
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
                extCallback: resolve,
            };
            showPermissionModal(notification, toolTitle).catch((err) => {
                console.error('[permissions] showPermissionModal failed:', err);
                resolve(false);
            });
        });
    }

    return {
        init,
        show: showPermissionModal,
        hide: hidePermissionModal,
        showForExtensionTool,
        /** Get the current permission request (for session-scoping in chat) */
        getCurrentRequest() {
            return currentPermissionRequest;
        },
    };
}

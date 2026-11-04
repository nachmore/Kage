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

import { getToolEmoji, escapeHtml, LTR_SLOT, setTextWithLtrValue } from './tool-utils.js';
import { EVT } from './events.js';
import { t } from './i18n.js';
import { cleanShellCommand, extractShellCommand } from './shell-commands.js';

/** ACP tool kind for shell / CLI commands (see src/config/permissions.rs). */
export const TOOL_KIND_EXECUTE = 'execute';

/** The ACP tool kind of a permission notification, as resolved by the backend. */
export function toolKindOf(notification) {
    return notification?._kageToolKind || notification?.params?.toolCall?.kind || null;
}

/**
 * The MCP server a permission notification's tool belongs to, as identified
 * by the backend (src/commands/messaging/notifications.rs). Never parsed
 * from the title here: the backend's rules (shell runs are never MCP,
 * ambiguous names are refused) are the only source of truth.
 */
export function mcpServerOf(notification) {
    const server = notification?._kageMcpServer;
    return typeof server === 'string' && server ? server : null;
}

/**
 * The two inline "trust a whole category" confirmations: every CLI command
 * (ACP kind "execute"), and every tool of one MCP server.
 */
const TRUST_PANELS = Object.freeze({
    commands: Object.freeze({
        panel: 'permissionTrustConfirm',
        title: 'permissionTrustConfirmTitle',
        body: 'permissionTrustConfirmBody',
        cancel: 'permissionTrustCancel',
        confirm: 'permissionTrustConfirmBtn',
    }),
    server: Object.freeze({
        panel: 'permissionTrustServerConfirm',
        title: 'permissionTrustServerConfirmTitle',
        body: 'permissionTrustServerConfirmBody',
        cancel: 'permissionTrustServerCancel',
        confirm: 'permissionTrustServerConfirmBtn',
    }),
});

/** Dropdown item that opens each confirmation. */
const TRUST_ITEMS = Object.freeze({
    commands: 'permissionTrustCommands',
    server: 'permissionTrustServer',
});

/**
 * Show the command line of a shell-run request (kind "execute") under the
 * title, in the same terminal style as the response's command blocks.
 * kiro-cli titles the request `Running: <command>` and Claude Code's
 * adapter titles it with the backticked command; such a title only repeats
 * the block, so it's hidden (the element keeps its text). Other requests
 * get no block. The command is untrusted: text nodes only.
 */
export function renderPermissionCommand(titleEl, toolKind, toolCall) {
    if (!titleEl) return;
    const title = typeof toolCall?.title === 'string' ? toolCall.title : '';
    const raw =
        toolKind === TOOL_KIND_EXECUTE ? extractShellCommand(toolCall?.rawInput, title) : '';
    const command = raw ? cleanShellCommand(raw) : '';
    let box = document.getElementById('permissionToolCommand');
    titleEl.classList.remove('is-shell-redundant');
    if (!command) {
        box?.remove();
        return;
    }
    if (!box) {
        box = document.createElement('pre');
        box.id = 'permissionToolCommand';
        box.className = 'shell-cmd-inline';
        box.dir = 'ltr';
        box.tabIndex = 0;
        box.setAttribute('role', 'group');
        titleEl.after(box);
    }
    box.setAttribute('aria-label', t('shared.shell.command_label'));
    box.textContent = '';
    const prompt = document.createElement('span');
    prompt.className = 'shell-cmd-prompt';
    prompt.setAttribute('aria-hidden', 'true');
    prompt.textContent = '$';
    box.append(prompt, document.createTextNode(command));
    if (extractShellCommand(null, title) === raw) titleEl.classList.add('is-shell-redundant');
}

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
            toolKind: toolKindOf(notification),
            mcpServer: notification.extCallback ? null : mcpServerOf(notification),
            isExtension: !!notification.extCallback,
            // Inline assist, macros, …: category trust never covers these
            // sessions (see the backend), so neither trust is offered.
            isBackground: notification._kageBackground === true,
            extCallback: notification.extCallback || null,
            notification,
        };

        // "Trust all commands…" only makes sense for an agent's shell run.
        const trustItem = document.getElementById(TRUST_ITEMS.commands);
        if (trustItem) {
            const offer = !!trustPlan('commands', currentPermissionRequest);
            trustItem.style.display = offer ? '' : 'none';
        }
        // "Trust all tools from <server>…" only for a tool the backend
        // identified as an MCP server's.
        const serverItem = document.getElementById(TRUST_ITEMS.server);
        if (serverItem) {
            const server = trustPlan('server', currentPermissionRequest)
                ? currentPermissionRequest.mcpServer
                : null;
            serverItem.style.display = server ? '' : 'none';
            if (server) {
                setTextWithLtrValue(
                    serverItem,
                    t('permission.dialog.trust_server', { server: LTR_SLOT }),
                    server
                );
            }
        }
        setTrustConfirmVisible(null);

        toolTitleEl.textContent = toolCall.title || t('shared.permission.unknown_tool');
        renderPermissionCommand(toolTitleEl, currentPermissionRequest.toolKind, toolCall);

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

    /**
     * Show one trust confirmation (`'commands'` / `'server'`) in place of
     * the action row, or neither (`null`). Always resets busy / error state.
     */
    function setTrustConfirmVisible(mode) {
        const actions = document.querySelector('#permissionModal .permission-actions');
        let shown = null;
        for (const [name, ids] of Object.entries(TRUST_PANELS)) {
            const panel = document.getElementById(ids.panel);
            if (!panel) continue;
            const visible = name === mode;
            panel.style.display = visible ? 'block' : 'none';
            if (visible) shown = ids;
        }
        if (actions) actions.style.display = shown ? 'none' : '';
        setTrustBusy(false);
        resetTrustText();
        // Focus the safe choice so a stray Enter can't trust everything.
        if (shown) document.getElementById(shown.cancel)?.focus();
    }

    /** Restore the confirmations' warning text (clearing a failure message). */
    function resetTrustText() {
        const body = document.getElementById(TRUST_PANELS.commands.body);
        if (body?.dataset.defaultText) {
            body.textContent = body.dataset.defaultText;
            body.classList.remove('permission-trust-error');
        }
        // The server confirmation names the current request's server.
        const server = currentPermissionRequest?.mcpServer;
        if (!server) return;
        const ids = TRUST_PANELS.server;
        setTextWithLtrValue(
            document.getElementById(ids.title),
            t('permission.dialog.trust_server_title', { server: LTR_SLOT }),
            server
        );
        const serverBody = document.getElementById(ids.body);
        setTextWithLtrValue(
            serverBody,
            t('permission.dialog.trust_server_body', { server: LTR_SLOT }),
            server
        );
        serverBody?.classList.remove('permission-trust-error');
    }

    function setTrustBusy(busy) {
        for (const ids of Object.values(TRUST_PANELS)) {
            for (const id of [ids.cancel, ids.confirm]) {
                const b = document.getElementById(id);
                if (b) b.disabled = busy;
            }
        }
    }

    function showTrustError(mode) {
        const body = document.getElementById(TRUST_PANELS[mode]?.body);
        if (!body) return;
        if (mode === 'commands') {
            body.dataset.defaultText ||= body.textContent;
            body.textContent = t('permission.dialog.trust_commands_failed');
        } else {
            body.textContent = t('permission.dialog.trust_server_failed');
        }
        body.classList.add('permission-trust-error');
    }

    /** The trust confirmation currently shown (`'commands'` / `'server'`), or null. */
    function visibleTrustPanel() {
        for (const [name, ids] of Object.entries(TRUST_PANELS)) {
            if (document.getElementById(ids.panel)?.style.display === 'block') return name;
        }
        return null;
    }

    /**
     * What confirming `mode` means for `req`: the backend command that
     * records the category grant, and which queued requests it covers.
     * Null when that category doesn't apply to the request.
     */
    function trustPlan(mode, req) {
        if (!req || req.isExtension || req.isBackground) return null;
        if (mode === 'commands' && req.toolKind === TOOL_KIND_EXECUTE) {
            return {
                command: 'set_tool_kind_trusted',
                args: { kind: TOOL_KIND_EXECUTE, trusted: true },
                covers: (n) => toolKindOf(n) === TOOL_KIND_EXECUTE,
            };
        }
        if (mode === 'server' && req.mcpServer) {
            const server = req.mcpServer;
            return {
                command: 'set_mcp_server_trusted',
                args: { server, trusted: true },
                covers: (n) => mcpServerOf(n) === server,
            };
        }
        return null;
    }

    /**
     * The user confirmed "Trust all commands" / "Trust all tools from
     * <server>": record the category grant, approve this request, and
     * approve the requests of that category already queued in this window
     * (they were asked before the trust existed — the backend auto-approves
     * only new ones). The backend decides each queued request again before
     * approving it, so one whose tool the user set to Deny meanwhile, or
     * one from a background session, keeps asking.
     */
    async function trustCategoryAndAllow(mode) {
        const req = currentPermissionRequest;
        const plan = trustPlan(mode, req);
        if (!plan) return;
        if (req.answering || req.trusting) return; // double-click / key repeat
        req.trusting = true;
        setTrustBusy(true);
        try {
            await invoke(plan.command, plan.args);
        } catch (e) {
            console.error(`Failed to save trust (${plan.command}):`, e);
            req.trusting = false;
            if (currentPermissionRequest === req) {
                setTrustBusy(false);
                showTrustError(mode);
            }
            return;
        }
        // The prompt may have moved on while we waited (answered elsewhere,
        // dismissed): never approve whatever request happens to be current
        // now — it could be an edit, an extension tool or another session's.
        if (currentPermissionRequest !== req) return;
        await approveCoveredQueued(plan);
        if (currentPermissionRequest !== req) return;
        await handlePermissionResponse('allow_once');
    }

    /**
     * Ask the backend to approve the queued requests `plan` covers. It
     * approves only those its policy allows now and returns their ids;
     * the rest stay queued for the user to answer.
     */
    async function approveCoveredQueued(plan) {
        const candidates = _permissionQueue.filter(
            (q) =>
                !q.notification?.extCallback &&
                !q.notification?._kageBackground &&
                plan.covers(q.notification)
        );
        if (candidates.length === 0) return;
        let approved;
        try {
            approved = await invoke('approve_permissions_if_allowed', {
                requestIds: candidates.map((q) => q.notification.id),
            });
        } catch (e) {
            console.error('Failed to approve queued requests:', e);
            return;
        }
        const done = new Set(
            (Array.isArray(approved) ? approved : []).map((id) => JSON.stringify(id))
        );
        _permissionQueue = _permissionQueue.filter(
            (q) => !done.has(JSON.stringify(q.notification?.id))
        );
    }

    /**
     * Escape / an outside click on a trust confirmation: back out, unless
     * the trust is being saved. Backing out then would show the Allow /
     * Deny row while the save can still succeed — the user could deny this
     * request and still end up trusting the whole category.
     */
    function backOutOfTrust() {
        if (currentPermissionRequest?.trusting) return;
        setTrustConfirmVisible(null);
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

        // Trust all commands / all tools from an MCP server: confirm inline
        // first (dangerous, category-wide).
        for (const [mode, ids] of Object.entries(TRUST_PANELS)) {
            document.getElementById(TRUST_ITEMS[mode])?.addEventListener('click', (e) => {
                e.stopPropagation();
                if (dropdown) dropdown.style.display = 'none';
                setTrustConfirmVisible(mode);
            });
            document
                .getElementById(ids.cancel)
                ?.addEventListener('click', () => setTrustConfirmVisible(null));
            document
                .getElementById(ids.confirm)
                ?.addEventListener('click', () => trustCategoryAndAllow(mode));
        }
    }

    function wireOverlayDismiss() {
        document.getElementById('permissionModal')?.addEventListener('click', (e) => {
            if (
                e.target.id === 'permissionModal' ||
                e.target === document.getElementById('permissionModal')
            ) {
                // Like Escape: an outside click backs out of the trust
                // confirmation rather than denying the request.
                if (visibleTrustPanel()) backOutOfTrust();
                else handlePermissionResponse('reject_once');
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
                const trustPanel = visibleTrustPanel();
                if (e.key === 'Escape') {
                    e.preventDefault();
                    e.stopPropagation();
                    if (trustPanel) backOutOfTrust();
                    else handlePermissionResponse('reject_once');
                } else if (e.key === 'Tab' && trustPanel) {
                    // Keep focus inside the confirmation (two buttons).
                    e.preventDefault();
                    e.stopPropagation();
                    const ids = TRUST_PANELS[trustPanel];
                    const cancel = document.getElementById(ids.cancel);
                    const confirmBtn = document.getElementById(ids.confirm);
                    (document.activeElement === cancel ? confirmBtn : cancel)?.focus();
                } else if (
                    trustPanel &&
                    (e.key === 'Enter' || e.key === ' ') &&
                    e.target instanceof HTMLButtonElement &&
                    e.target.closest(`#${TRUST_PANELS[trustPanel].panel}`)
                ) {
                    // Keyboard use of the trust confirmation's own buttons.
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

            // Ride on the notification so it survives queueing.
            notification._kageToolKind =
                event.payload.toolKind || notification.params?.toolCall?.kind || null;
            notification._kageMcpServer =
                typeof event.payload.mcpServer === 'string' ? event.payload.mcpServer : null;
            notification._kageBackground = event.payload.backgroundSession === true;

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

    const sessionOf = (q) => q.notification?.params?.sessionId || '';

    /**
     * Bring a queued request for `sessionId` forward when its session becomes
     * the visible one. Without this, a request queued behind another
     * session's (hidden) prompt stayed invisible on switching back, and only
     * surfaced once that other prompt was answered — in the wrong session's
     * view. The currently-hidden request is requeued by showPermissionModal.
     * Returns true if one was shown.
     */
    function showQueuedForSession(sessionId) {
        if (!sessionId) return false;
        const modal = document.getElementById('permissionModal');
        if (currentPermissionRequest && isShowing(modal)) return false;
        const idx = _permissionQueue.findIndex((q) => sessionOf(q) === sessionId);
        if (idx < 0) return false;
        const [next] = _permissionQueue.splice(idx, 1);
        showPermissionModal(next.notification, next.toolName);
        return true;
    }

    return {
        init,
        show: showPermissionModal,
        hide: hidePermissionModal,
        showForExtensionTool,
        showQueuedForSession,
        /** Get the current permission request (for session-scoping in chat) */
        getCurrentRequest() {
            return currentPermissionRequest;
        },
        /** True if the current or any queued request belongs to `sessionId`. */
        hasRequestForSession(sessionId) {
            if (currentPermissionRequest?.sessionId === sessionId) return true;
            return _permissionQueue.some((q) => sessionOf(q) === sessionId);
        },
    };
}

/**
 * "Trust all commands" in the shared permission modal: offered only for
 * shell runs (ACP kind "execute"), confirmed inline, and on confirm it
 * records the category grant, approves the current request and any command
 * prompts already queued in the window.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createPermissionHandler } from '../../ui/js/shared/permissions-core.js';

function mountModal() {
    document.body.innerHTML = `
        <div id="permissionModal" style="display: none">
            <div class="permission-modal">
                <div id="permissionToolName"></div>
                <p id="permissionToolTitle"></p>
                <div class="permission-actions">
                    <button id="permissionDeny"></button>
                    <button id="permissionDenyMenu"></button>
                    <div id="permissionDenyDropdown" style="display:none"><button id="permissionDenyAlways"></button></div>
                    <button id="permissionAllow"></button>
                    <button id="permissionAllowMenu"></button>
                    <div id="permissionAllowDropdown" style="display:none">
                        <button id="permissionAllow24h"></button>
                        <button id="permissionAllowAlways"></button>
                        <button id="permissionTrustCommands" style="display:none"></button>
                    </div>
                </div>
                <div id="permissionTrustConfirm" style="display:none">
                    <p id="permissionTrustConfirmBody">warning</p>
                    <button id="permissionTrustCancel"></button>
                    <button id="permissionTrustConfirmBtn"></button>
                </div>
            </div>
        </div>`;
}

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));
const el = (id) => document.getElementById(id);

// Each handler wires document-level listeners; remove them between tests.
const docListeners = [];
const realAdd = document.addEventListener.bind(document);
beforeEach(() => {
    vi.spyOn(document, 'addEventListener').mockImplementation((...args) => {
        docListeners.push(args);
        return realAdd(...args);
    });
});
afterEach(async () => {
    // The handler shows the next queued request on a 150 ms timer; let it
    // fire while the DOM still exists (it otherwise lands after teardown
    // on slow runners: "document is not defined").
    await new Promise((r) => setTimeout(r, 200)); // drainQueuedPermissionTimers
    for (const [type, fn, opts] of docListeners.splice(0)) document.removeEventListener(type, fn, opts);
    vi.restoreAllMocks();
});

function setup() {
    mountModal();
    const listeners = {};
    const appWindow = { listen: vi.fn((evt, fn) => { listeners[evt] = fn; }) };
    // The backend approves every queued candidate it's asked about unless
    // a test says otherwise (it re-decides each against the policy), and
    // names those it approved by their permission tokens.
    const invoke = vi.fn(async (cmd, args) =>
        cmd === 'approve_permissions_if_allowed' ? args.permissionTokens : undefined
    );
    const handler = createPermissionHandler(invoke, appWindow);
    handler.init();
    const request = async (
        id,
        { kind = null, title = `Running: cmd ${id}`, sessionId = 's1', token = `tok-${id}` } = {}
    ) => {
        await listeners.permission_request({
            payload: {
                notification: { id, params: { sessionId, toolCall: { title }, options: [] } },
                auto_approve: false,
                toolName: title,
                toolKind: kind,
                permissionToken: token,
            },
        });
    };
    return { invoke, handler, request, listeners };
}

describe('Permission tokens (a restarted agent reuses request ids)', () => {
    it('withdrawing the old request leaves a new one with the same id on screen', async () => {
        const { request, listeners, invoke } = setup();
        await request(5, { token: 'new' });
        // The restart's withdrawal of the OLD process's request 5.
        listeners.permission_dismissed({ payload: { requestId: 5, permissionToken: 'old' } });
        expect(el('permissionModal').style.display).toBe('flex');
        // Its own dismissal closes it.
        listeners.permission_dismissed({ payload: { requestId: 5, permissionToken: 'new' } });
        await tick();
        expect(el('permissionModal').style.display).toBe('none');
        expect(invoke).not.toHaveBeenCalledWith('send_permission_response', expect.anything());
    });

    // R3: an id alone can name another process's request.
    it('a dismissal without a token closes nothing', async () => {
        const { request, listeners } = setup();
        await request(5);
        await request(6); // queued
        listeners.permission_dismissed({ payload: { requestId: 5 } });
        listeners.permission_dismissed({ payload: { requestId: 6 } });
        await tick();
        expect(el('permissionModal').style.display).toBe('flex');
        expect(el('permissionToolTitle').textContent).toBe('Running: cmd 5');
        // Its own dismissal still closes it, and the queued one shows.
        listeners.permission_dismissed({ payload: { requestId: 5, permissionToken: 'tok-5' } });
        await tick(200);
        expect(el('permissionToolTitle').textContent).toBe('Running: cmd 6');
    });

    it('a dismissal naming only a token removes the queued request with it', async () => {
        const { request, listeners } = setup();
        await request(5);
        await request(6); // queued
        listeners.permission_dismissed({ payload: { requestId: 6, permissionToken: 'tok-6' } });
        listeners.permission_dismissed({ payload: { requestId: 5, permissionToken: 'tok-5' } });
        await tick(200);
        expect(el('permissionModal').style.display).toBe('none');
    });

    it('a broadcast dismissal (no id, no token) still drops everything', async () => {
        const { request, listeners } = setup();
        await request(5);
        await request(6);
        listeners.permission_dismissed({ payload: {} });
        await tick(200);
        expect(el('permissionModal').style.display).toBe('none');
    });

    // R3: a restart replaced queued request 2 while the category approval
    // was in flight; the new request 2 has its own token and must stay.
    it('a category approval never drops a new request that took an approved id', async () => {
        const { request, listeners, invoke } = setup();
        let approve;
        invoke.mockImplementation(async (cmd) => {
            if (cmd === 'approve_permissions_if_allowed') {
                return new Promise((r) => {
                    approve = r;
                });
            }
        });
        await request(1, { kind: 'execute' });
        await request(2, { kind: 'execute', token: 'old-2' }); // queued
        el('permissionTrustCommands').click();
        el('permissionTrustConfirmBtn').click();
        await tick();
        expect(invoke).toHaveBeenCalledWith('approve_permissions_if_allowed', {
            requestIds: [2],
            permissionTokens: ['old-2'],
        });

        // Meanwhile the agent restarts: the old request 2 is withdrawn, and
        // the new process asks its own request 2.
        listeners.permission_dismissed({ payload: { requestId: 2, permissionToken: 'old-2' } });
        await request(2, { kind: 'edit', title: 'Editing new.rs', token: 'new-2' });

        // The backend approved the old request 2 (what it returns names it).
        approve(['old-2']);
        await tick(250);
        await tick(200);

        // Request 1 was answered; the new request 2 is asked, not dropped.
        const answered = invoke.mock.calls
            .filter(([cmd]) => cmd === 'send_permission_response')
            .map(([, a]) => [a.requestId, a.permissionToken]);
        expect(answered).toEqual([[1, 'tok-1']]);
        expect(el('permissionModal').style.display).toBe('flex');
        expect(el('permissionToolTitle').textContent).toBe('Editing new.rs');
    });

    it('an approval result naming ids, not tokens, removes nothing', async () => {
        const { request, invoke } = setup();
        invoke.mockImplementation(async (cmd, args) =>
            cmd === 'approve_permissions_if_allowed' ? args.requestIds : undefined
        );
        await request(1, { kind: 'execute' });
        await request(2, { kind: 'execute', title: 'Running: cmd two' }); // queued
        el('permissionTrustCommands').click();
        el('permissionTrustConfirmBtn').click();
        await tick(250);
        await tick(200);
        expect(el('permissionModal').style.display).toBe('flex');
        expect(el('permissionToolTitle').textContent).toBe('Running: cmd two');
    });

    it('the answer carries the token the request came with', async () => {
        const { request, invoke } = setup();
        await request(9, { token: 'tok-nine' });
        el('permissionDeny').click();
        await tick(150);
        expect(invoke).toHaveBeenCalledWith(
            'send_permission_response',
            expect.objectContaining({ requestId: 9, permissionToken: 'tok-nine', optionId: 'reject_once' })
        );
    });
});

describe('Trust all commands', () => {
    it('is offered for shell runs only', async () => {
        const { request, handler } = setup();
        await request(1, { kind: 'execute' });
        expect(el('permissionTrustCommands').style.display).toBe('');
        await handler.hide();
        await tick(200);
        await request(2, { kind: 'edit', title: 'Editing main.rs' });
        expect(el('permissionTrustCommands').style.display).toBe('none');
    });

    it('is never offered for extension tools', async () => {
        const { handler } = setup();
        handler.showForExtensionTool('todos', 'add', '✅');
        await tick();
        expect(el('permissionTrustCommands').style.display).toBe('none');
    });

    it('confirms inline before doing anything, and Cancel backs out', async () => {
        const { request, invoke } = setup();
        await request(1, { kind: 'execute' });
        el('permissionTrustCommands').click();
        expect(el('permissionTrustConfirm').style.display).toBe('block');
        expect(document.querySelector('.permission-actions').style.display).toBe('none');
        expect(document.activeElement).toBe(el('permissionTrustCancel'));
        expect(invoke).not.toHaveBeenCalled();
        el('permissionTrustCancel').click();
        expect(el('permissionTrustConfirm').style.display).toBe('none');
        expect(document.querySelector('.permission-actions').style.display).toBe('');
        expect(invoke).not.toHaveBeenCalled();
    });

    it('Escape on the confirmation backs out instead of denying', async () => {
        const { request, invoke } = setup();
        await request(1, { kind: 'execute' });
        el('permissionTrustCommands').click();
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        expect(el('permissionTrustConfirm').style.display).toBe('none');
        expect(invoke).not.toHaveBeenCalledWith('send_permission_response', expect.anything());
    });

    it('confirming trusts the kind, allows this request and queued commands only', async () => {
        const { request, invoke } = setup();
        await request(1, { kind: 'execute' });
        await request(2, { kind: 'execute' }); // queued behind #1
        await request(3, { kind: 'edit', title: 'Editing a.rs' }); // queued, not a command
        el('permissionTrustCommands').click();
        el('permissionTrustConfirmBtn').click();
        await tick(250);

        expect(invoke).toHaveBeenCalledWith('set_tool_kind_trusted', { kind: 'execute', trusted: true });
        const answered = invoke.mock.calls
            .filter(([cmd]) => cmd === 'send_permission_response')
            .map(([, a]) => [a.requestId, a.permissionToken, a.optionId]);
        expect(answered).toEqual([[1, 'tok-1', 'allow_once']]);
        // Queued commands go to the backend, which re-decides each, by the
        // token it named each with.
        expect(invoke).toHaveBeenCalledWith('approve_permissions_if_allowed', {
            requestIds: [2],
            permissionTokens: ['tok-2'],
        });
        // The edit request is shown next instead.
        await tick(200);
        expect(el('permissionToolTitle').textContent).toBe('Editing a.rs');
    });

    it('never approves a different request that became current during the save', async () => {
        const { request, invoke, handler } = setup();
        let release;
        invoke.mockImplementation(async (cmd) => {
            if (cmd === 'set_tool_kind_trusted') await new Promise((r) => { release = r; });
        });
        await request(1, { kind: 'execute' });
        await request(2, { kind: 'edit', title: 'Editing a.rs' }); // queued
        el('permissionTrustCommands').click();
        el('permissionTrustConfirmBtn').click();
        await tick();
        // Request 1 is answered elsewhere while the save is in flight.
        await handler.hide();
        await tick(200); // the edit request is now current
        release();
        await tick(250);
        const answered = invoke.mock.calls.filter(([c]) => c === 'send_permission_response');
        expect(answered).toEqual([]);
        expect(el('permissionToolTitle').textContent).toBe('Editing a.rs');
    });

    it('a queued command the backend refuses (e.g. set to Deny meanwhile) keeps asking', async () => {
        const { request, invoke } = setup();
        invoke.mockImplementation(async (cmd) =>
            cmd === 'approve_permissions_if_allowed' ? [] : undefined
        );
        await request(1, { kind: 'execute' });
        await request(2, { kind: 'execute', title: 'Running: rm -rf build' });
        el('permissionTrustCommands').click();
        el('permissionTrustConfirmBtn').click();
        await tick(250);
        await tick(200);
        expect(el('permissionToolTitle').textContent).toBe('Running: rm -rf build');
    });

    it('a double click trusts and answers once', async () => {
        const { request, invoke } = setup();
        await request(1, { kind: 'execute' });
        el('permissionTrustCommands').click();
        el('permissionTrustConfirmBtn').click();
        el('permissionTrustConfirmBtn').click();
        await tick(250);
        expect(invoke.mock.calls.filter(([c]) => c === 'set_tool_kind_trusted')).toHaveLength(1);
        expect(invoke.mock.calls.filter(([c]) => c === 'send_permission_response')).toHaveLength(1);
    });

    it('an outside click backs out of the confirmation instead of denying', async () => {
        const { request, invoke } = setup();
        await request(1, { kind: 'execute' });
        el('permissionTrustCommands').click();
        el('permissionModal').click();
        expect(el('permissionTrustConfirm').style.display).toBe('none');
        expect(invoke).not.toHaveBeenCalledWith('send_permission_response', expect.anything());
    });

    it('Tab keeps focus inside the confirmation', async () => {
        const { request } = setup();
        await request(1, { kind: 'execute' });
        el('permissionTrustCommands').click();
        expect(document.activeElement).toBe(el('permissionTrustCancel'));
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));
        expect(document.activeElement).toBe(el('permissionTrustConfirmBtn'));
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));
        expect(document.activeElement).toBe(el('permissionTrustCancel'));
    });

    it('does not answer the request if saving the trust fails', async () => {
        const { request, invoke } = setup();
        invoke.mockImplementation(async (cmd) => {
            if (cmd === 'set_tool_kind_trusted') throw new Error('disk full');
        });
        vi.spyOn(console, 'error').mockImplementation(() => {});
        await request(1, { kind: 'execute' });
        el('permissionTrustCommands').click();
        el('permissionTrustConfirmBtn').click();
        await tick(150);
        expect(invoke).not.toHaveBeenCalledWith('send_permission_response', expect.anything());
        expect(el('permissionModal').style.display).toBe('flex');
        // The confirmation stays up with an error and usable buttons.
        expect(el('permissionTrustConfirm').style.display).toBe('block');
        expect(el('permissionTrustConfirmBody').classList.contains('permission-trust-error')).toBe(true);
        expect(el('permissionTrustConfirmBtn').disabled).toBe(false);
        // Backing out restores the original warning text.
        el('permissionTrustCancel').click();
        expect(el('permissionTrustConfirmBody').textContent).toBe('warning');
    });
});

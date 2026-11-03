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
afterEach(() => {
    for (const [type, fn, opts] of docListeners.splice(0)) document.removeEventListener(type, fn, opts);
    vi.restoreAllMocks();
});

function setup() {
    mountModal();
    const listeners = {};
    const appWindow = { listen: vi.fn((evt, fn) => { listeners[evt] = fn; }) };
    const invoke = vi.fn(async () => undefined);
    const handler = createPermissionHandler(invoke, appWindow);
    handler.init();
    const request = async (id, { kind = null, title = `Running: cmd ${id}`, sessionId = 's1' } = {}) => {
        await listeners.permission_request({
            payload: {
                notification: { id, params: { sessionId, toolCall: { title }, options: [] } },
                auto_approve: false,
                toolName: title,
                toolKind: kind,
            },
        });
    };
    return { invoke, handler, request };
}

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
            .map(([, a]) => [a.requestId, a.optionId]);
        expect(answered).toEqual(expect.arrayContaining([[1, 'allow_once'], [2, 'allow_once']]));
        expect(answered.find(([id]) => id === 3)).toBeUndefined();
        // The edit request is shown next instead.
        await tick(200);
        expect(el('permissionToolTitle').textContent).toBe('Editing a.rs');
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
    });
});

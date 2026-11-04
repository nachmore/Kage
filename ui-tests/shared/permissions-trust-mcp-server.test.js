/**
 * "Trust all tools from <server>" in the shared permission modal: offered
 * only when the backend identified the tool's MCP server (`mcpServer` in the
 * permission_request payload), confirmed inline with server-specific
 * wording, and on confirm it records the server grant, approves the current
 * request and any queued requests from the same server.
 */
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { createPermissionHandler } from '../../ui/js/shared/permissions-core.js';
import { initI18n } from '../../ui/js/shared/i18n.js';

const EN_CATALOG = JSON.parse(
    readFileSync(
        join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'locales', 'en', 'messages.json'),
        'utf-8'
    )
);

beforeAll(async () => {
    // Real EN strings, so the {server} placeholder is actually rendered.
    await initI18n(async () => ({
        language: 'en',
        rtl: false,
        catalog: EN_CATALOG,
        fallback: EN_CATALOG,
    }));
});

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
                        <button id="permissionTrustServer" style="display:none"></button>
                    </div>
                </div>
                <div id="permissionTrustConfirm" style="display:none">
                    <p id="permissionTrustConfirmBody">warning</p>
                    <button id="permissionTrustCancel"></button>
                    <button id="permissionTrustConfirmBtn"></button>
                </div>
                <div id="permissionTrustServerConfirm" style="display:none">
                    <p id="permissionTrustServerConfirmTitle"></p>
                    <p id="permissionTrustServerConfirmBody"></p>
                    <button id="permissionTrustServerCancel"></button>
                    <button id="permissionTrustServerConfirmBtn"></button>
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
    const request = async (
        id,
        {
            server = null,
            kind = null,
            title = server ? `Running: @${server}/Tool${id}` : `Running: cmd ${id}`,
            sessionId = 's1',
            background = false,
        } = {}
    ) => {
        await listeners.permission_request({
            payload: {
                notification: { id, params: { sessionId, toolCall: { title }, options: [] } },
                auto_approve: false,
                toolName: title,
                toolKind: kind,
                mcpServer: server,
                backgroundSession: background,
            },
        });
    };
    return { invoke, handler, request };
}

const answered = (invoke) =>
    invoke.mock.calls
        .filter(([cmd]) => cmd === 'send_permission_response')
        .map(([, a]) => [a.requestId, a.optionId]);

describe('Trust all tools from an MCP server', () => {
    it('is offered only when the backend names the server, with the name LTR-isolated', async () => {
        const { request, handler } = setup();
        await request(1, { server: 'builder-mcp' });
        const item = el('permissionTrustServer');
        expect(item.style.display).toBe('');
        expect(item.textContent).toBe('Trust all tools from builder-mcp…');
        const name = item.querySelector('bdi');
        expect(name.textContent).toBe('builder-mcp');
        expect(name.getAttribute('dir')).toBe('ltr');
        // Not a shell run: no "Trust all commands".
        expect(el('permissionTrustCommands').style.display).toBe('none');

        await handler.hide();
        await tick(200);
        // A title that merely looks like an MCP call: the frontend never
        // parses titles — no backend server, no offer.
        await request(2, { title: 'Running: @builder-mcp/InternalSearch', kind: 'execute' });
        expect(el('permissionTrustServer').style.display).toBe('none');
        expect(el('permissionTrustCommands').style.display).toBe('');
    });

    it('renders a server name as text, never as markup', async () => {
        const { request } = setup();
        // The backend validates names; this checks the DOM path regardless.
        await request(1, { server: '<img src=x onerror=alert(1)>' });
        expect(el('permissionTrustServer').querySelector('img')).toBeNull();
        expect(el('permissionTrustServer').querySelector('bdi').textContent).toBe(
            '<img src=x onerror=alert(1)>'
        );
    });

    it('is never offered for extension tools', async () => {
        const { handler } = setup();
        handler.showForExtensionTool('todos', 'add', '✅');
        await tick();
        expect(el('permissionTrustServer').style.display).toBe('none');
    });

    it('confirms inline with the server named, and Cancel backs out', async () => {
        const { request, invoke } = setup();
        await request(1, { server: 'builder-mcp' });
        el('permissionTrustServer').click();
        expect(el('permissionTrustServerConfirm').style.display).toBe('block');
        expect(el('permissionTrustConfirm').style.display).toBe('none');
        expect(document.querySelector('.permission-actions').style.display).toBe('none');
        expect(document.activeElement).toBe(el('permissionTrustServerCancel'));
        expect(el('permissionTrustServerConfirmTitle').textContent).toBe(
            'Trust every tool from builder-mcp?'
        );
        const body = el('permissionTrustServerConfirmBody');
        expect(body.querySelector('bdi').textContent).toBe('builder-mcp');
        expect(body.textContent).toContain('any it adds later');
        expect(invoke).not.toHaveBeenCalled();

        el('permissionTrustServerCancel').click();
        expect(el('permissionTrustServerConfirm').style.display).toBe('none');
        expect(document.querySelector('.permission-actions').style.display).toBe('');
        expect(invoke).not.toHaveBeenCalled();
    });

    it('Escape and an outside click back out instead of denying', async () => {
        const { request, invoke } = setup();
        await request(1, { server: 'builder-mcp' });
        el('permissionTrustServer').click();
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        expect(el('permissionTrustServerConfirm').style.display).toBe('none');
        el('permissionTrustServer').click();
        el('permissionModal').click();
        expect(el('permissionTrustServerConfirm').style.display).toBe('none');
        expect(invoke).not.toHaveBeenCalledWith('send_permission_response', expect.anything());
        expect(el('permissionModal').style.display).toBe('flex');
    });

    it('Tab keeps focus inside the confirmation', async () => {
        const { request } = setup();
        await request(1, { server: 'builder-mcp' });
        el('permissionTrustServer').click();
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));
        expect(document.activeElement).toBe(el('permissionTrustServerConfirmBtn'));
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));
        expect(document.activeElement).toBe(el('permissionTrustServerCancel'));
    });

    it('confirming trusts the server and allows this and queued requests from it only', async () => {
        const { request, invoke } = setup();
        await request(1, { server: 'builder-mcp' });
        await request(2, { server: 'builder-mcp' }); // queued, same server
        await request(3, { server: 'kage-computer-control' }); // queued, other server
        await request(4, { kind: 'execute' }); // queued shell command
        await request(5, { server: 'builder-mcp', background: true }); // background session
        await request(6, { server: 'builder-mcp', sessionId: 's2' }); // other visible session
        el('permissionTrustServer').click();
        el('permissionTrustServerConfirmBtn').click();
        await tick(250);

        expect(invoke).toHaveBeenCalledWith('set_mcp_server_trusted', {
            server: 'builder-mcp',
            trusted: true,
        });
        expect(invoke).not.toHaveBeenCalledWith('set_tool_kind_trusted', expect.anything());
        const ids = answered(invoke).map(([id]) => id);
        expect(answered(invoke)).toEqual(
            expect.arrayContaining([
                [1, 'allow_once'],
                [2, 'allow_once'],
                [6, 'allow_once'],
            ])
        );
        expect(ids).not.toContain(3);
        expect(ids).not.toContain(4);
        // Category trust never covers background sessions: still asks.
        expect(ids).not.toContain(5);
        // The other server's request is shown next.
        await tick(200);
        expect(el('permissionToolTitle').textContent).toBe('Running: @kage-computer-control/Tool3');
    });

    it('never approves a different request that became current during the save', async () => {
        const { request, invoke, handler } = setup();
        let release;
        invoke.mockImplementation(async (cmd) => {
            if (cmd === 'set_mcp_server_trusted') await new Promise((r) => { release = r; });
        });
        await request(1, { server: 'builder-mcp' });
        await request(2, { server: 'builder-mcp' }); // queued, same server
        el('permissionTrustServer').click();
        el('permissionTrustServerConfirmBtn').click();
        await tick();
        // Request 1 is answered elsewhere while the save is in flight.
        await handler.hide();
        await tick(200); // request 2 is now current
        release();
        await tick(250);
        expect(answered(invoke)).toEqual([]);
        expect(el('permissionToolTitle').textContent).toBe('Running: @builder-mcp/Tool2');
    });

    it('a double click trusts and answers once', async () => {
        const { request, invoke } = setup();
        await request(1, { server: 'builder-mcp' });
        el('permissionTrustServer').click();
        el('permissionTrustServerConfirmBtn').click();
        el('permissionTrustServerConfirmBtn').click();
        await tick(250);
        expect(invoke.mock.calls.filter(([c]) => c === 'set_mcp_server_trusted')).toHaveLength(1);
        expect(answered(invoke)).toHaveLength(1);
    });

    it('does not answer the request if saving the trust fails', async () => {
        const { request, invoke } = setup();
        invoke.mockImplementation(async (cmd) => {
            if (cmd === 'set_mcp_server_trusted') throw new Error('disk full');
        });
        vi.spyOn(console, 'error').mockImplementation(() => {});
        await request(1, { server: 'builder-mcp' });
        el('permissionTrustServer').click();
        el('permissionTrustServerConfirmBtn').click();
        await tick(150);
        expect(answered(invoke)).toEqual([]);
        expect(el('permissionModal').style.display).toBe('flex');
        const body = el('permissionTrustServerConfirmBody');
        expect(el('permissionTrustServerConfirm').style.display).toBe('block');
        expect(body.classList.contains('permission-trust-error')).toBe(true);
        expect(body.textContent).toContain("Couldn't save that setting");
        expect(el('permissionTrustServerConfirmBtn').disabled).toBe(false);
        // Backing out and reopening restores the server warning.
        el('permissionTrustServerCancel').click();
        el('permissionTrustServer').click();
        expect(body.classList.contains('permission-trust-error')).toBe(false);
        expect(body.querySelector('bdi').textContent).toBe('builder-mcp');
    });

    it('keeps the server across a hide-and-requeue on session switch', async () => {
        const { request, handler } = setup();
        await request(1, { server: 'builder-mcp', sessionId: 's1' });
        // A session switch hides the modal without answering it.
        el('permissionModal').style.display = 'none';
        await request(2, { kind: 'execute', sessionId: 's2' });
        expect(el('permissionTrustServer').style.display).toBe('none');
        await handler.hide();
        await tick(200);
        expect(handler.getCurrentRequest().mcpServer).toBe('builder-mcp');
        expect(el('permissionTrustServer').style.display).toBe('');
    });
});

describe('Trust all commands (category approvals)', () => {
    it('leaves queued commands from background sessions asking', async () => {
        const { request, invoke } = setup();
        await request(1, { kind: 'execute' });
        await request(2, { kind: 'execute', background: true });
        await request(3, { kind: 'execute' });
        el('permissionTrustCommands').click();
        el('permissionTrustConfirmBtn').click();
        await tick(250);
        const ids = answered(invoke).map(([id]) => id);
        expect(ids).toEqual(expect.arrayContaining([1, 3]));
        expect(ids).not.toContain(2);
    });
});

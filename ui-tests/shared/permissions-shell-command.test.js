/**
 * The permission prompt shows a shell run's command (ACP kind "execute") in
 * the terminal-style command box, as text, and leaves other requests alone.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
    createPermissionHandler,
    renderPermissionCommand,
} from '../../ui/js/shared/permissions-core.js';

function mountModal() {
    document.body.innerHTML = `
        <div id="permissionModal" style="display: none">
            <div class="permission-modal">
                <div class="permission-body">
                    <div id="permissionToolName"></div>
                    <p id="permissionToolTitle" class="permission-tool-title"></p>
                </div>
                <div class="permission-actions">
                    <button id="permissionDeny"></button>
                    <button id="permissionAllow"></button>
                </div>
            </div>
        </div>`;
}

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));
const el = (id) => document.getElementById(id);

const docListeners = [];
const realAdd = document.addEventListener.bind(document);
beforeEach(() => {
    vi.spyOn(document, 'addEventListener').mockImplementation((...args) => {
        docListeners.push(args);
        return realAdd(...args);
    });
    delete window.__pwned;
});
afterEach(() => {
    for (const [type, fn, opts] of docListeners.splice(0)) {
        document.removeEventListener(type, fn, opts);
    }
    vi.restoreAllMocks();
});

function setup() {
    mountModal();
    const listeners = {};
    const appWindow = {
        listen: vi.fn((evt, fn) => {
            listeners[evt] = fn;
        }),
    };
    const invoke = vi.fn(async () => undefined);
    const handler = createPermissionHandler(invoke, appWindow);
    handler.init();
    const request = async (id, { kind = null, title, rawInput } = {}) => {
        await listeners.permission_request({
            payload: {
                notification: {
                    id,
                    params: { sessionId: 's1', toolCall: { title, rawInput }, options: [] },
                },
                auto_approve: false,
                toolName: 'shell',
                toolKind: kind,
            },
        });
    };
    return { handler, request };
}

describe('permission prompt command box', () => {
    it('shows an execute request’s command from rawInput in the command style', async () => {
        const { request } = setup();
        await request(1, {
            kind: 'execute',
            title: 'Running: rm -rf build',
            rawInput: { command: 'rm -rf build' },
        });
        const box = el('permissionToolCommand');
        expect(box).not.toBeNull();
        expect(box.classList.contains('shell-cmd-inline')).toBe(true);
        expect(box.dir).toBe('ltr');
        expect(box.textContent).toBe('$rm -rf build');
        expect(box.getAttribute('aria-label')).toBe('shared.shell.command_label');
        // Right after the title, which keeps its text (and is only hidden
        // because it repeats the command).
        expect(el('permissionToolTitle').nextElementSibling).toBe(box);
        expect(el('permissionToolTitle').textContent).toBe('Running: rm -rf build');
        expect(el('permissionToolTitle').classList.contains('is-shell-redundant')).toBe(true);
    });

    it('falls back to the title, and keeps a title that says more than the command', async () => {
        const { request, handler } = setup();
        await request(1, { kind: 'execute', title: 'Running: git push' });
        expect(el('permissionToolCommand').textContent).toBe('$git push');
        await handler.hide();
        await tick(200);
        await request(2, {
            kind: 'execute',
            title: 'Deploy the site',
            rawInput: { command: 'npm run deploy' },
        });
        expect(el('permissionToolCommand').textContent).toBe('$npm run deploy');
        expect(el('permissionToolTitle').classList.contains('is-shell-redundant')).toBe(false);
    });

    it('shows nothing extra for other tools, and removes a previous box', async () => {
        const { request, handler } = setup();
        await request(1, { kind: 'execute', title: 'Running: ls' });
        expect(el('permissionToolCommand')).not.toBeNull();
        await handler.hide();
        await tick(200);
        await request(2, { kind: 'edit', title: 'Editing a.rs', rawInput: { command: 'create' } });
        expect(el('permissionToolCommand')).toBeNull();
        expect(el('permissionToolTitle').textContent).toBe('Editing a.rs');
        expect(el('permissionToolTitle').classList.contains('is-shell-redundant')).toBe(false);
    });

    it('never shows a kiro MCP title as a command, even with kind execute missing', async () => {
        const { request } = setup();
        await request(1, { kind: null, title: 'Running: @builder-mcp/InternalSearch' });
        expect(el('permissionToolCommand')).toBeNull();
    });

    it('renders the command as text and strips terminal escapes', () => {
        mountModal();
        const title = el('permissionToolTitle');
        title.textContent = 'x';
        renderPermissionCommand(title, 'execute', {
            title: 'x',
            rawInput: { command: '\x1b[31mecho\x1b[0m <img src=x onerror="window.__pwned=1">' },
        });
        const box = el('permissionToolCommand');
        expect(box.querySelector('img')).toBeNull();
        expect(box.textContent).toBe('$echo <img src=x onerror="window.__pwned=1">');
        expect(window.__pwned).toBeUndefined();
    });
});

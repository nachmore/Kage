/**
 * Queueing and visibility in the shared permission modal core, focused on
 * chat's session switch, which hides the modal while its request stays
 * pending.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createPermissionHandler } from '../../ui/js/shared/permissions-core.js';

function mountModal() {
    document.body.innerHTML = `
        <div id="permissionModal" style="display: none">
            <div id="permissionToolTitle"></div>
            <div id="permissionToolName"></div>
            <button id="permissionDeny"></button>
            <button id="permissionAllow"></button>
        </div>`;
    return document.getElementById('permissionModal');
}

function acpRequest(id, sessionId) {
    return {
        id,
        params: { sessionId, toolCall: { title: `tool ${id}` }, options: [] },
    };
}

// MutationObserver callbacks are microtasks.
const flush = () => new Promise((r) => setTimeout(r, 0));

describe('permissions-core session-switch handling', () => {
    let modal;
    let handler;
    let invoke;

    beforeEach(() => {
        modal = mountModal();
        invoke = vi.fn(async () => undefined);
        handler = createPermissionHandler(invoke, { listen: vi.fn() });
        handler.init();
    });

    afterEach(() => {
        vi.useRealTimers();
    });

    it('shows a new request instead of queueing it behind a hidden one', async () => {
        await handler.show(acpRequest(1, 'A'));
        modal.style.display = 'none'; // chat switched to session B
        await handler.show(acpRequest(2, 'B'));

        expect(handler.getCurrentRequest().id).toBe(2);
        expect(modal.style.display).toBe('flex');
    });

    it('brings the hidden request back once the new one is answered', async () => {
        vi.useFakeTimers();
        await handler.show(acpRequest(1, 'A'));
        modal.style.display = 'none';
        await handler.show(acpRequest(2, 'B'));

        document.getElementById('permissionAllow').click();
        await vi.advanceTimersByTimeAsync(400);

        expect(invoke).toHaveBeenCalledWith(
            'send_permission_response',
            expect.objectContaining({ requestId: 2 })
        );
        expect(handler.getCurrentRequest()?.id).toBe(1);
    });

    it('keeps an extension-tool prompt visible across a session switch', async () => {
        const answer = handler.showForExtensionTool('ext', 'tool', '*');
        await flush();
        modal.style.display = 'none';
        await flush();
        expect(modal.style.display).toBe('flex');

        document.getElementById('permissionDeny').click();
        await expect(answer).resolves.toBe(false);
    });

    it('does not swallow typing while the pending request is hidden', async () => {
        await handler.show(acpRequest(1, 'A'));
        modal.style.display = 'none';

        const e = new KeyboardEvent('keydown', { key: 'a', cancelable: true });
        document.dispatchEvent(e);
        expect(e.defaultPrevented).toBe(false);
    });
});

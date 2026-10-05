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

// Each handler wires document-level listeners. They look the modal up by
// id, so a previous test's handler would react to the next test's modal;
// remove them after every test.
const docListeners = [];
const realAddEventListener = document.addEventListener.bind(document);
beforeEach(() => {
    vi.spyOn(document, 'addEventListener').mockImplementation((...args) => {
        docListeners.push(args);
        return realAddEventListener(...args);
    });
});
afterEach(() => {
    for (const [type, fn, opts] of docListeners.splice(0)) {
        document.removeEventListener(type, fn, opts);
    }
    vi.restoreAllMocks();
});

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

describe('permissions-core keyboard handling across session switches', () => {
    let modal;
    let handler;
    let invoke;

    // Mirrors chat's window.ChatPermissions.onSessionSwitch: hide the
    // modal (request stays pending) for another session, re-show it for
    // the request's own session.
    function switchTo(sessionId) {
        const req = handler.getCurrentRequest();
        if (!req) return;
        modal.style.display = req.sessionId === sessionId ? 'flex' : 'none';
    }

    function press(key, target = document) {
        const e = new KeyboardEvent('keydown', { key, cancelable: true, bubbles: true });
        target.dispatchEvent(e);
        return e;
    }

    const responses = () => invoke.mock.calls.filter(([cmd]) => cmd === 'send_permission_response');

    beforeEach(() => {
        modal = mountModal();
        invoke = vi.fn(async () => undefined);
        handler = createPermissionHandler(invoke, { listen: vi.fn() });
        handler.init();
    });

    afterEach(() => {
        vi.useRealTimers();
    });

    it('blocks typing and Enter while the prompt is visible', async () => {
        await handler.show(acpRequest(1, 'A'));

        expect(press('a').defaultPrevented).toBe(true);
        expect(press('Enter').defaultPrevented).toBe(true);
        expect(responses()).toHaveLength(0);
    });

    it('denies on Escape while the prompt is visible', async () => {
        vi.useFakeTimers();
        await handler.show(acpRequest(1, 'A'));

        expect(press('Escape').defaultPrevented).toBe(true);
        await vi.advanceTimersByTimeAsync(200);

        expect(responses()).toHaveLength(1);
        expect(responses()[0][1]).toEqual(
            expect.objectContaining({ requestId: 1, optionId: 'reject_once' })
        );
        expect(handler.getCurrentRequest()).toBeNull();
    });

    it('lets Escape, Enter and typing through in another session', async () => {
        await handler.show(acpRequest(1, 'A'));
        switchTo('B');

        const input = document.createElement('input');
        document.body.appendChild(input);
        const seen = [];
        input.addEventListener('keydown', (e) => seen.push(e.key));

        for (const key of ['a', 'Enter', 'Escape']) {
            expect(press(key, input).defaultPrevented).toBe(false);
        }
        expect(seen).toEqual(['a', 'Enter', 'Escape']);
        // Escape in session B must not deny session A's hidden request.
        expect(responses()).toHaveLength(0);
        expect(handler.getCurrentRequest()?.id).toBe(1);
    });

    it('resumes keyboard handling after switching back to the session', async () => {
        vi.useFakeTimers();
        await handler.show(acpRequest(1, 'A'));
        switchTo('B');
        expect(press('a').defaultPrevented).toBe(false);

        switchTo('A');
        expect(modal.style.display).toBe('flex');
        expect(press('a').defaultPrevented).toBe(true);

        press('Escape');
        await vi.advanceTimersByTimeAsync(200);
        expect(responses()).toHaveLength(1);
        expect(responses()[0][1]).toEqual(expect.objectContaining({ requestId: 1 }));
    });

    it('keeps blocking keys for an extension prompt across a session switch', async () => {
        const answer = handler.showForExtensionTool('ext', 'tool', '*');
        await flush();
        switchTo('B'); // no session id: chat hides it, the observer re-shows it
        await flush();

        expect(press('a').defaultPrevented).toBe(true);
        press('Escape');
        await expect(answer).resolves.toBe(false);
    });
});

import { describe, it, expect, beforeEach } from 'vitest';
import { renderToolbarButtons } from '../../ui/js/shared/extension-toolbar.js';

// Minimal sanitizer stub: returns a text node, mirroring how the real
// extension sanitizer treats a plain-text / emoji icon.
const textIcon = (s) => document.createTextNode(s);

function makeButton(overrides = {}) {
    return {
        id: 'btn-1',
        extensionId: 'ext-1',
        tooltip: 'Do a thing',
        icon: '🔧',
        onClick: () => ({}),
        ...overrides,
    };
}

describe('renderToolbarButtons', () => {
    let container;
    beforeEach(() => {
        container = document.createElement('div');
    });

    it('is a no-op for a null container', () => {
        expect(() =>
            renderToolbarButtons({
                container: null,
                buttons: [makeButton()],
                buttonClass: 'x',
                sanitizeIcon: textIcon,
                buildContext: () => ({}),
                onHostEffect: () => {},
            })
        ).not.toThrow();
    });

    it('renders one button per descriptor with the marker + window class', () => {
        renderToolbarButtons({
            container,
            buttons: [makeButton({ id: 'a' }), makeButton({ id: 'b' })],
            buttonClass: 'chat-toolbar-btn',
            sanitizeIcon: textIcon,
            buildContext: () => ({}),
            onHostEffect: () => {},
        });
        const btns = container.querySelectorAll('button');
        expect(btns.length).toBe(2);
        expect(btns[0].classList.contains('chat-toolbar-btn')).toBe(true);
        expect(btns[0].classList.contains('ext-toolbar-btn')).toBe(true);
    });

    it('uses the tooltip as the title, falling back to id', () => {
        renderToolbarButtons({
            container,
            buttons: [makeButton({ tooltip: '', id: 'fallback-id' })],
            buttonClass: 'x',
            sanitizeIcon: textIcon,
            buildContext: () => ({}),
            onHostEffect: () => {},
        });
        expect(container.querySelector('button').title).toBe('fallback-id');
    });

    it('falls back to the wrench icon when icon is missing', () => {
        renderToolbarButtons({
            container,
            buttons: [makeButton({ icon: undefined })],
            buttonClass: 'x',
            sanitizeIcon: textIcon,
            buildContext: () => ({}),
            onHostEffect: () => {},
        });
        expect(container.querySelector('button').textContent).toBe('🔧');
    });

    it('clears previously-rendered extension buttons but leaves native ones', () => {
        const native = document.createElement('button');
        native.className = 'chat-toolbar-btn native';
        container.appendChild(native);

        renderToolbarButtons({
            container,
            buttons: [makeButton()],
            buttonClass: 'chat-toolbar-btn',
            sanitizeIcon: textIcon,
            buildContext: () => ({}),
            onHostEffect: () => {},
        });
        // Re-render: the previous ext button should be removed, native kept.
        renderToolbarButtons({
            container,
            buttons: [makeButton()],
            buttonClass: 'chat-toolbar-btn',
            sanitizeIcon: textIcon,
            buildContext: () => ({}),
            onHostEffect: () => {},
        });

        expect(container.querySelectorAll('.ext-toolbar-btn').length).toBe(1);
        expect(container.contains(native)).toBe(true);
    });

    it('passes the built context to onClick and routes host effects', async () => {
        let seenCtx = null;
        let seenHost = null;
        let seenBtn = null;
        renderToolbarButtons({
            container,
            buttons: [makeButton({ onClick: (ctx) => ({ host: { type: 'set_chat_input', value: ctx.input } }) })],
            buttonClass: 'x',
            sanitizeIcon: textIcon,
            buildContext: (btn) => {
                seenBtn = btn;
                return { input: 'hello' };
            },
            onHostEffect: (host) => {
                seenHost = host;
            },
        });

        container.querySelector('button').click();
        // Let the async click handler settle.
        await Promise.resolve();
        await Promise.resolve();

        expect(seenBtn?.id).toBe('btn-1');
        expect(seenHost).toEqual({ type: 'set_chat_input', value: 'hello' });
        seenCtx = seenHost.value;
        expect(seenCtx).toBe('hello');
    });

    it('isolates a throwing onClick — no throw escapes, other buttons still wired', async () => {
        let goodClicked = false;
        renderToolbarButtons({
            container,
            buttons: [
                makeButton({
                    id: 'bad',
                    onClick: () => {
                        throw new Error('boom');
                    },
                }),
                makeButton({ id: 'good', onClick: () => { goodClicked = true; return {}; } }),
            ],
            buttonClass: 'x',
            sanitizeIcon: textIcon,
            buildContext: () => ({}),
            onHostEffect: () => {},
        });

        const [bad, good] = container.querySelectorAll('button');
        expect(() => bad.click()).not.toThrow();
        good.click();
        await Promise.resolve();
        expect(goodClicked).toBe(true);
    });

    it('does not call onHostEffect when onClick returns no host', async () => {
        let hostCalls = 0;
        renderToolbarButtons({
            container,
            buttons: [makeButton({ onClick: () => ({}) })],
            buttonClass: 'x',
            sanitizeIcon: textIcon,
            buildContext: () => ({}),
            onHostEffect: () => {
                hostCalls++;
            },
        });
        container.querySelector('button').click();
        await Promise.resolve();
        await Promise.resolve();
        expect(hostCalls).toBe(0);
    });
});

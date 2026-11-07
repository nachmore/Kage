/**
 * A widget refresh that renders exactly what is already on screen leaves
 * the widget's DOM alone, so keyboard focus (and a selection, or text typed
 * into the widget) survives the tick. Anything that changes the painted
 * result (html, className, actions, the host's date-picker wiring, the UI
 * language) repaints, and so does every render after the host replaced or
 * hid the widget's content (hidden, paused notice, retry, stale clear, a
 * paint that threw).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

const VIEW = {
    html: '<button data-ext-action="next" class="w-btn">Next</button><span>3 due</span>',
    actions: [{ id: 'next', rpc: 'next' }],
};

function makeController(overrides = {}) {
    const host = document.createElement('div');
    document.body.appendChild(host);
    return {
        extensionId: 'test-ext',
        widgetId: 'w1',
        slot: 'floating-bottom',
        host,
        timer: null,
        destroyed: false,
        refreshIntervalMs: 0,
        renderInFlight: false,
        consecutiveFailures: 0,
        tripped: false,
        lastSuccessRenderAt: 0,
        paintedSignature: null,
        ...overrides,
    };
}

/** A manager whose widget renders whatever `view()` returns now. */
function makeHarness(initial = VIEW) {
    let current = initial;
    const sandbox = {
        widgetIds: ['w1'],
        call: vi.fn(async (method) => {
            if (method === 'renderWidget') {
                if (current instanceof Error) throw current;
                return typeof current === 'function' ? current() : current;
            }
            return null;
        }),
    };
    const mgr = new ExtensionManager(async () => undefined);
    const ext = { manifest: { id: 'test-ext' }, capabilities: [], sandbox };
    mgr.extensions.set('test-ext', ext);
    const ctrl = makeController();
    const render = () => mgr._renderWidget(ctrl);
    const set = (view) => {
        current = view;
    };
    const actionCalls = () =>
        sandbox.call.mock.calls.filter(([m]) => m === 'onWidgetAction').map(([, p]) => p.actionId);
    return { mgr, ext, sandbox, ctrl, render, set, actionCalls };
}

beforeEach(() => {
    document.body.innerHTML = '';
});
afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
});

describe('widget identical-render skipping', () => {
    it('keeps the DOM and keyboard focus when a refresh renders the same view', async () => {
        const h = makeHarness();
        await h.render();
        const btn = h.ctrl.host.querySelector('button');
        btn.focus();
        expect(document.activeElement).toBe(btn);

        await h.render();
        await h.render({ ...VIEW, actions: [{ id: 'next', rpc: 'next' }] });

        expect(h.ctrl.host.querySelector('button')).toBe(btn);
        expect(document.activeElement).toBe(btn);
        expect(h.ctrl.consecutiveFailures).toBe(0);
        // Still wired, and wired once.
        btn.click();
        await Promise.resolve();
        expect(h.actionCalls()).toEqual(['next']);
    });

    it('repaints when the html changes', async () => {
        const h = makeHarness();
        await h.render();
        const before = h.ctrl.host.querySelector('button');
        h.set({ ...VIEW, html: VIEW.html.replace('3 due', '2 due') });
        await h.render();
        expect(h.ctrl.host.querySelector('button')).not.toBe(before);
        expect(h.ctrl.host.textContent).toContain('2 due');
    });

    it('repaints when only the actions change, and routes clicks to the new rpc', async () => {
        const h = makeHarness();
        await h.render();
        const before = h.ctrl.host.querySelector('button');
        h.set({ ...VIEW, actions: [{ id: 'next', rpc: 'next-page' }] });
        await h.render();
        const after = h.ctrl.host.querySelector('button');
        expect(after).not.toBe(before);
        after.click();
        await Promise.resolve();
        expect(h.actionCalls()).toEqual(['next-page']);
    });

    it('repaints when the className changes', async () => {
        const h = makeHarness();
        await h.render();
        h.set({ ...VIEW, className: 'w-urgent' });
        await h.render();
        expect(h.ctrl.host.classList.contains('w-urgent')).toBe(true);
    });

    it('shows the view again after a hidden (null) render', async () => {
        const h = makeHarness();
        await h.render();
        h.set(null);
        await h.render();
        expect(h.ctrl.host.style.display).toBe('none');
        expect(h.ctrl.host.innerHTML).toBe('');
        h.set(VIEW);
        await h.render();
        expect(h.ctrl.host.style.display).toBe('');
        expect(h.ctrl.host.textContent).toContain('3 due');
    });

    it('repaints the same view after the paused notice and a retry', async () => {
        const h = makeHarness();
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        await h.render();
        h.set(new Error('boom'));
        await h.render();
        await h.render();
        await h.render();
        expect(h.ctrl.tripped).toBe(true);
        expect(h.ctrl.host.querySelector('.ext-widget-paused')).toBeTruthy();

        h.set(VIEW); // the very view painted before the trip
        await h.mgr._retryWidget(h.ctrl);
        expect(h.ctrl.tripped).toBe(false);
        expect(h.ctrl.host.querySelector('.ext-widget-paused')).toBeNull();
        expect(h.ctrl.host.textContent).toContain('3 due');
    });

    it('repaints the same view after stale content was cleared', async () => {
        const h = makeHarness();
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        await h.render();
        // Painted long ago, then a failed render clears it as stale.
        h.ctrl.lastSuccessRenderAt = performance.now() - 10 * 60_000;
        h.set(new Error('offline'));
        await h.render();
        expect(h.ctrl.host.innerHTML).toBe('');
        h.set(VIEW);
        await h.render();
        expect(h.ctrl.host.textContent).toContain('3 due');
    });

    it('repaints the same view after a paint that threw part-way', async () => {
        const h = makeHarness();
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        vi.spyOn(h.mgr, '_wireDateAffordances').mockImplementationOnce(() => {
            throw new Error('wiring failed');
        });
        await h.render();
        expect(h.ctrl.consecutiveFailures).toBe(1);
        await h.render();
        expect(h.ctrl.consecutiveFailures).toBe(0);
        const btn = h.ctrl.host.querySelector('button');
        btn.click();
        await Promise.resolve();
        expect(h.actionCalls()).toEqual(['next']);
    });

    it('repaints when the window starts offering the date picker', async () => {
        const h = makeHarness({ html: '<button data-kage-date="2026-11-06">Fri, Nov 6</button>' });
        h.ext.capabilities = ['calendar'];
        h.ext.sandbox.hasDateProvider = true;
        await h.render();
        // No picker handler yet: the date label is plain text.
        expect(h.ctrl.host.querySelector('button')).toBeNull();

        const handler = vi.fn();
        h.mgr.setDateAffordanceHandler(handler);
        await h.render();
        const btn = h.ctrl.host.querySelector('button.kage-date-affordance');
        expect(btn).toBeTruthy();
        btn.click();
        expect(handler).toHaveBeenCalledWith(
            expect.objectContaining({ date: '2026-11-06', extensionId: 'test-ext' })
        );
        // And from then on the same view is kept.
        await h.render();
        expect(h.ctrl.host.querySelector('button.kage-date-affordance')).toBe(btn);
    });

    it('always repaints output it cannot compare, without failing the render', async () => {
        const h = makeHarness({ ...VIEW, actions: [{ id: 'next', rpc: 10n }] });
        await h.render();
        const before = h.ctrl.host.querySelector('button');
        await h.render();
        expect(h.ctrl.host.querySelector('button')).not.toBe(before);
        expect(h.ctrl.consecutiveFailures).toBe(0);
    });
});

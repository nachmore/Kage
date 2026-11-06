import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ExtensionManager } from '../../ui/js/shared/extension-manager.js';

let managers = [];
beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout', 'performance'] });
    document.body.innerHTML = '';
    delete window._kageFloatingHidden;
});
afterEach(() => {
    managers.forEach((mgr) => mgr.destroy());
    managers = [];
    vi.useRealTimers();
    vi.restoreAllMocks();
    document.body.innerHTML = '';
    delete window._kageFloatingHidden;
});

function fixture(interval = 10_000) {
    const config = { extension_states: { ext: true }, extensions: { ext: { interval } } };
    const mgr = new ExtensionManager(vi.fn(async (cmd) => cmd === 'get_config' ? config : null));
    mgr._configCache = structuredClone(config);
    managers.push(mgr);
    let ms = interval;
    const render = vi.fn(async () => ({ html: '<div>watchlist</div>' }));
    const readInterval = vi.fn(async () => ms);
    const sandbox = {
        widgetIds: ['watchlist'],
        updateConfig: vi.fn(async () => {}),
        call: vi.fn(async (method) => {
            if (method === 'getWidgetRefreshInterval') return readInterval();
            if (method === 'renderWidget') return render();
            return null;
        }),
    };
    const widget = { id: 'watchlist', slot: 'floating-bottom', module: './widget.js' };
    const ext = { manifest: { id: 'ext', contributes: { widgets: [widget] } }, sandbox };
    mgr.extensions.set('ext', ext);
    const slot = document.createElement('div');
    document.body.appendChild(slot);
    const mount = async () => {
        mgr.setWidgetSlot(widget.slot, slot);
        await vi.advanceTimersByTimeAsync(0);
        return mgr._widgetInstances.get('ext:watchlist');
    };
    return { mgr, ext, widget, config, slot, mount, render, readInterval, setInterval: (value) => {
        ms = value;
        config.extensions.ext.interval = value;
    } };
}

describe('widget slots and refresh lifecycle', () => {
    it('creates no controllers or refresh RPCs in a chat window without slots', async () => {
        const f = fixture();
        await f.mgr._mountAllWidgets();
        await f.mgr._onConfigUpdateOnce();
        await f.mgr.renderAllWidgets();
        await vi.advanceTimersByTimeAsync(120_000);
        expect(f.mgr._widgetInstances.size).toBe(0);
        expect(f.render).not.toHaveBeenCalled();
        expect(f.readInterval).not.toHaveBeenCalled();
        expect(vi.getTimerCount()).toBe(0);
    });

    it('mounts when its slot arrives late and never mounts unrelated slots', async () => {
        const f = fixture();
        await f.mgr._mountAllWidgets();
        f.mgr.setWidgetSlot('floating-status', f.slot);
        await vi.advanceTimersByTimeAsync(0);
        expect(f.render).not.toHaveBeenCalled();
        const ctrl = await f.mount();
        expect(ctrl.host.parentElement).toBe(f.slot);
        expect(f.render).toHaveBeenCalledTimes(1);
        await vi.advanceTimersByTimeAsync(10_000);
        expect(f.render).toHaveBeenCalledTimes(2);
    });

    it('does no initial refresh while hidden; shows missing content immediately on resume', async () => {
        const f = fixture();
        f.mgr.setWidgetVisibility(false);
        const ctrl = await f.mount();
        expect(ctrl.timer).toBeNull();
        expect(f.render).not.toHaveBeenCalled();
        await vi.advanceTimersByTimeAsync(60_000);
        await f.mgr.setWidgetVisibility(true);
        expect(f.render).toHaveBeenCalledTimes(1);
        expect(ctrl.host.textContent).toBe('watchlist');
        expect(ctrl.timer).not.toBeNull();
    });

    it('stops all timers while hidden and refreshes stale content once on show', async () => {
        const f = fixture();
        const ctrl = await f.mount();
        await f.mgr.setWidgetVisibility(false);
        expect(ctrl.timer).toBeNull();
        expect(vi.getTimerCount()).toBe(0);
        await vi.advanceTimersByTimeAsync(120_000);
        expect(f.render).toHaveBeenCalledTimes(1);
        await f.mgr.setWidgetVisibility(true);
        expect(f.render).toHaveBeenCalledTimes(2);
        expect(ctrl.consecutiveFailures).toBe(0);
        await vi.advanceTimersByTimeAsync(10_000);
        expect(f.render).toHaveBeenCalledTimes(3);
    });

    it('resumes the timer without spending a refresh on still-fresh content', async () => {
        const f = fixture();
        await f.mount();
        await f.mgr.setWidgetVisibility(false);
        await vi.advanceTimersByTimeAsync(500);
        await f.mgr.setWidgetVisibility(true);
        expect(f.render).toHaveBeenCalledTimes(1);
        await vi.advanceTimersByTimeAsync(10_000);
        expect(f.render).toHaveBeenCalledTimes(2);
    });

    it('forces a fresh repaint after one second without postponing the 15-second poll', async () => {
        const f = fixture(15_000);
        await vi.advanceTimersByTimeAsync(1);
        const ctrl = await f.mount();
        const timer = ctrl.timer;
        await vi.advanceTimersByTimeAsync(1_000);
        expect(ctrl.lastSuccessRenderAt).toBe(1);
        // A real successful paint (hasRendered), rather than a seeded empty controller.
        await f.mgr.renderAllWidgets({ force: true });
        expect(f.render).toHaveBeenCalledTimes(2);
        expect(ctrl.lastSuccessRenderAt).toBe(1_001);
        expect(ctrl.timer).toBe(timer);
        await vi.advanceTimersByTimeAsync(13_999);
        expect(f.render).toHaveBeenCalledTimes(2);
        await vi.advanceTimersByTimeAsync(1);
        expect(f.render).toHaveBeenCalledTimes(3);
    });

    it.each([60_000, 300_000])('refreshes %ims content on show after five seconds, debouncing rapid summons', async (interval) => {
        const f = fixture(interval);
        await f.mount();
        await f.mgr.setWidgetVisibility(false);
        await vi.advanceTimersByTimeAsync(5_000);
        await f.mgr.setWidgetVisibility(true);
        expect(f.render).toHaveBeenCalledTimes(2);
        for (let i = 0; i < 10; i++) {
            await f.mgr.setWidgetVisibility(false);
            await vi.advanceTimersByTimeAsync(100);
            await f.mgr.setWidgetVisibility(true);
        }
        expect(f.render).toHaveBeenCalledTimes(2);
        expect(vi.getTimerCount()).toBe(1);
    });

    it('does no widget or sandbox config work for an unrelated save or equivalent effective config', async () => {
        const f = fixture();
        await f.mount();
        f.config.tool_permissions = { server: { trusted: true } };
        await f.mgr.onConfigUpdate();
        f.config.extensions.ext = { enabled: false, __scratch: 'ignored', interval: 10_000 };
        await f.mgr.onConfigUpdate();
        expect(f.render).toHaveBeenCalledTimes(1);
        expect(f.readInterval).toHaveBeenCalledTimes(1);
        expect(f.ext.sandbox.updateConfig).not.toHaveBeenCalled();
    });

    it('does not refresh when nested config objects only change key order', async () => {
        const f = fixture();
        f.config.extensions.ext.options = { a: 1, b: [2] };
        f.mgr._configCache = structuredClone(f.config);
        await f.mount();
        await f.mgr.onConfigUpdate();
        f.config.extensions.ext.options = { b: [2], a: 1 };
        await f.mgr.onConfigUpdate();
        expect(f.render).toHaveBeenCalledTimes(1);
        expect(f.readInterval).toHaveBeenCalledTimes(1);
        expect(f.ext.sandbox.updateConfig).not.toHaveBeenCalled();
    });

    it.each(['interval', 'render'])('does not await a pending widget %s RPC on a changed config', async (pending) => {
        const f = fixture();
        await f.mount();
        let release;
        const rpc = pending === 'interval' ? f.readInterval : f.render;
        rpc.mockImplementationOnce(() => new Promise((resolve) => { release = resolve; }));
        f.config.extensions.ext.label = 'changed';
        await f.mgr.onConfigUpdate();
        expect(release).toBeTypeOf('function');
        expect(f.ext.sandbox.updateConfig).toHaveBeenCalledTimes(1);
        release(pending === 'interval' ? 10_000 : { html: 'updated' });
        await vi.advanceTimersByTimeAsync(0);
        expect(f.render).toHaveBeenCalledTimes(2);
    });

    it('refreshes only the widget whose extension config changed', async () => {
        const f = fixture();
        await f.mount();
        const otherRender = vi.fn(async () => ({ html: 'other' }));
        const other = {
            manifest: { id: 'other', contributes: { widgets: [f.widget] } },
            sandbox: {
                widgetIds: ['watchlist'],
                updateConfig: vi.fn(async () => {}),
                call: vi.fn(async (method) => method === 'renderWidget' ? otherRender() : 10_000),
            },
        };
        f.mgr.extensions.set('other', other);
        await f.mgr._mountWidget('other', other, f.widget);
        f.config.extensions.ext.label = 'changed';
        await f.mgr.onConfigUpdate();
        await vi.advanceTimersByTimeAsync(0);
        expect(f.render).toHaveBeenCalledTimes(2);
        expect(otherRender).toHaveBeenCalledTimes(1);
        expect(other.sandbox.updateConfig).not.toHaveBeenCalled();
    });

    it('also pauses and resumes on document visibility changes', async () => {
        const f = fixture();
        const ctrl = await f.mount();
        const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
        document.dispatchEvent(new Event('visibilitychange'));
        expect(ctrl.timer).toBeNull();
        await vi.advanceTimersByTimeAsync(30_000);
        expect(f.render).toHaveBeenCalledTimes(1);
        hidden.mockReturnValue(false);
        document.dispatchEvent(new Event('visibilitychange'));
        await vi.advanceTimersByTimeAsync(0);
        expect(f.render).toHaveBeenCalledTimes(2);
    });

    it('re-reads and re-arms a changed interval on config update, including zero', async () => {
        const f = fixture(60_000);
        const ctrl = await f.mount();
        f.setInterval(2_000);
        await f.mgr._onConfigUpdateOnce();
        expect(ctrl.refreshIntervalMs).toBe(2_000);
        expect(f.render).toHaveBeenCalledTimes(2);
        await vi.advanceTimersByTimeAsync(2_000);
        expect(f.render).toHaveBeenCalledTimes(3);
        f.setInterval(0);
        await f.mgr._onConfigUpdateOnce();
        expect(ctrl.timer).toBeNull();
        expect(f.render).toHaveBeenCalledTimes(4);
        await vi.advanceTimersByTimeAsync(120_000);
        expect(f.render).toHaveBeenCalledTimes(4);
        f.setInterval(1_000);
        await f.mgr._onConfigUpdateOnce();
        await vi.advanceTimersByTimeAsync(1_000);
        expect(f.render).toHaveBeenCalledTimes(6);
    });

    it('defers a hidden config refresh until shown, even with interval zero', async () => {
        const f = fixture(0);
        await f.mount();
        await f.mgr.setWidgetVisibility(false);
        f.config.extensions.ext.label = 'changed';
        await f.mgr._onConfigUpdateOnce();
        expect(f.render).toHaveBeenCalledTimes(1);
        await f.mgr.setWidgetVisibility(true);
        expect(f.render).toHaveBeenCalledTimes(2);
        expect(vi.getTimerCount()).toBe(0);
    });

    it('does not lose a config refresh arriving during a render when the new interval is zero', async () => {
        const f = fixture(10_000);
        const ctrl = await f.mount();
        let release;
        let notifyStarted;
        const started = new Promise((resolve) => { notifyStarted = resolve; });
        f.render.mockImplementationOnce(() => new Promise((resolve) => {
            release = resolve;
            notifyStarted();
        }));
        const rendering = f.mgr._renderWidget(ctrl);
        await started;
        f.setInterval(0);
        await f.mgr._onConfigUpdateOnce();
        release({ html: '<div>old config</div>' });
        await rendering;
        await vi.advanceTimersByTimeAsync(0);
        expect(f.render).toHaveBeenCalledTimes(3);
        expect(ctrl.host.textContent).toBe('watchlist');
        expect(ctrl.timer).toBeNull();
    });

    it('does not arm an orphan timer if unloaded or hidden during interval lookup', async () => {
        for (const action of ['unload', 'hide']) {
            const f = fixture();
            let release;
            f.readInterval.mockImplementationOnce(() => new Promise((resolve) => { release = resolve; }));
            f.mgr._widgetSlots = new Map([[f.widget.slot, f.slot]]);
            const mounting = f.mgr._mountWidget('ext', f.ext, f.widget);
            if (action === 'unload') f.mgr._unmountWidgetsFor('ext');
            else await f.mgr.setWidgetVisibility(false);
            release(10_000);
            await mounting;
            expect(f.render).not.toHaveBeenCalled();
            expect(vi.getTimerCount()).toBe(0);
        }
    });

    it('keeps a hidden config change pending when an older render completes', async () => {
        const f = fixture(0);
        const ctrl = await f.mount();
        let release;
        f.render.mockImplementationOnce(() => new Promise((resolve) => { release = resolve; }));
        const rendering = f.mgr._renderWidget(ctrl);
        await f.mgr.setWidgetVisibility(false);
        f.config.extensions.ext.label = 'changed';
        await f.mgr._onConfigUpdateOnce();
        release({ html: '<div>old config</div>' });
        await rendering;
        expect(f.render).toHaveBeenCalledTimes(2);
        await f.mgr.setWidgetVisibility(true);
        expect(f.render).toHaveBeenCalledTimes(3);
        expect(ctrl.host.textContent).toBe('watchlist');
        expect(vi.getTimerCount()).toBe(0);
    });

    it('does not arm a timer if hidden during the initial render', async () => {
        const f = fixture();
        let release;
        let notifyStarted;
        const started = new Promise((resolve) => { notifyStarted = resolve; });
        f.render.mockImplementationOnce(() => new Promise((resolve) => {
            release = resolve;
            notifyStarted();
        }));
        f.mgr._widgetSlots = new Map([[f.widget.slot, f.slot]]);
        const mounting = f.mgr._mountWidget('ext', f.ext, f.widget);
        await started;
        await f.mgr.setWidgetVisibility(false);
        release({ html: '<div>late</div>' });
        await mounting;
        expect(vi.getTimerCount()).toBe(0);
    });

    it('keeps the interval bounded and never auto-resumes a tripped widget', async () => {
        const f = fixture(1);
        const ctrl = await f.mount();
        expect(ctrl.refreshIntervalMs).toBe(1_000);
        ctrl.tripped = true;
        await f.mgr.setWidgetVisibility(false);
        await f.mgr.setWidgetVisibility(true);
        expect(ctrl.timer).toBeNull();
        expect(f.render).toHaveBeenCalledTimes(1);
    });

    it('skips RPCs for detached widget hosts', async () => {
        const f = fixture();
        const ctrl = await f.mount();
        f.slot.remove();
        await f.mgr._renderWidget(ctrl);
        await vi.advanceTimersByTimeAsync(30_000);
        expect(f.render).toHaveBeenCalledTimes(1);
        expect(ctrl.timer).toBeNull();
    });

    it('cleans up timers and document listeners on destroy', async () => {
        const f = fixture();
        const ctrl = await f.mount();
        const remove = vi.spyOn(document, 'removeEventListener');
        f.mgr.destroy();
        expect(ctrl.timer).toBeNull();
        expect(remove).toHaveBeenCalledWith('visibilitychange', expect.any(Function));
        expect(vi.getTimerCount()).toBe(0);
    });
});

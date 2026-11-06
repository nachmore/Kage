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
    const config = { extension_states: { ext: true }, extensions: { ext: {} } };
    const mgr = new ExtensionManager(vi.fn(async (cmd) => cmd === 'get_config' ? config : null));
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
    return { mgr, ext, widget, config, slot, mount, render, readInterval, setInterval: (value) => { ms = value; } };
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

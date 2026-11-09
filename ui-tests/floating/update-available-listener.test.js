/**
 * The floating window's `update_available` listener: it offers an
 * "Install now" banner, except while `>update` (which raised the event with
 * its own check) is already installing. Then it hides any stale offer
 * instead, and leaves other banners alone.
 */

import { describe, it, expect, vi } from 'vitest';
import { FloatingApp } from '../../ui/js/floating/app.js';
import { matchCommands } from '../../ui/js/shared/commands.js';
import { EVT } from '../../ui/js/shared/events.js';

function registerListeners() {
    const handlers = {};
    const app = Object.create(FloatingApp.prototype);
    app.listen = (event, handler) => {
        handlers[event] = handler;
    };
    app.banner = { showText: vi.fn(), dismissUpdateOffer: vi.fn() };
    app._registerAppEventListeners();
    expect(handlers[EVT.UPDATE_AVAILABLE]).toBeTypeOf('function');
    return { app, onUpdateAvailable: handlers[EVT.UPDATE_AVAILABLE] };
}

describe('floating update_available listener', () => {
    it('offers Install now when >update is not running', () => {
        const { app, onUpdateAvailable } = registerListeners();
        onUpdateAvailable({ payload: '1.2.0' });
        expect(app.banner.showText).toHaveBeenCalledTimes(1);
        expect(app.banner.showText.mock.calls[0][3]).toBe('update_install');
        expect(app.banner.dismissUpdateOffer).not.toHaveBeenCalled();
    });

    it('holds the offer back while >update is checking or installing', async () => {
        const { app, onUpdateAvailable } = registerListeners();
        let finishCheck;
        const invoke = (name) => {
            expect(name).toBe('check_for_update');
            return new Promise((resolve) => {
                finishCheck = resolve;
            });
        };
        const update = matchCommands('>update').find((c) => c.name === 'update');
        const running = update.execute(invoke);

        onUpdateAvailable({ payload: '1.2.0' });
        expect(app.banner.showText).not.toHaveBeenCalled();
        expect(app.banner.dismissUpdateOffer).toHaveBeenCalledTimes(1);

        // Release the guard (no update found) so the offer comes back.
        finishCheck({ current_version: '1.0.0', available_version: null });
        await running;
        onUpdateAvailable({ payload: '1.2.0' });
        expect(app.banner.showText).toHaveBeenCalledTimes(1);
    });
});

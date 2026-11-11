/**
 * On macOS, "include selected text" needs Accessibility, which ad-hoc
 * signed builds lose on every update. The backend then skips the capture
 * and tells the launcher, which says why once per run (when no other
 * banner is up) and offers to open Settings → macOS Permissions.
 */
import { describe, it, expect, vi } from 'vitest';

vi.mock('../../ui/js/shared/i18n.js', async (importOriginal) => ({
    ...(await importOriginal()),
    t: (key) => key,
}));

const { LifecycleEventsMethods } = await import('../../ui/js/floating/app/lifecycle-events.js');
const { EVT } = await import('../../ui/js/shared/events.js');

function setup({ bannerVisible = false } = {}) {
    const handlers = {};
    const app = {
        listen: (name, cb) => {
            handlers[name] = cb;
        },
        banner: { visible: bannerVisible, showText: vi.fn() },
    };
    LifecycleEventsMethods.setupStreamingListeners.call(app);
    return { app, fire: () => handlers[EVT.SELECTION_CAPTURE_UNAVAILABLE]() };
}

describe('selected text without Accessibility', () => {
    it('explains it once per run, with a button to the permissions page', () => {
        const { app, fire } = setup();
        fire();
        fire();
        expect(app.banner.showText).toHaveBeenCalledTimes(1);
        expect(app.banner.showText).toHaveBeenCalledWith(
            '🔒',
            'floating.banner.selection_needs_accessibility',
            'floating.banner.action.fix_permission',
            'settings',
            'mac-permissions'
        );
    });

    it('waits while another banner is showing', () => {
        const { app, fire } = setup({ bannerVisible: true });
        fire();
        expect(app.banner.showText).not.toHaveBeenCalled();
        app.banner.visible = false;
        fire();
        expect(app.banner.showText).toHaveBeenCalledTimes(1);
    });
});

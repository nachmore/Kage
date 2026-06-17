// Floating-window banner system: the dismissible strip at the top of the
// content area used for "Kage was updated", "Kage crashed last session",
// update-available / installing notices, and extension-driven banners.
//
// Extracted from FloatingApp into a small controller so the ~135 lines of
// banner state + DOM no longer live in the window god class. The app wires
// it up once with a few callbacks (the things only the app knows: window
// resize, UI reset, whether a response is streaming) and otherwise calls
// `show()` / `dismiss()` / `checkForUpdateBanner()` / `checkForCrashBanner()`.

import { t } from '../shared/i18n.js';
import { formatError } from '../shared/session-render.js';

/**
 * Resolve a banner action descriptor (`{ type, data }`) into a concrete
 * intent the controller should carry out. Pure — no DOM, no I/O — so the
 * routing rules are unit-testable. Returns one of:
 *   { kind: 'settings', section, subSection? }
 *   { kind: 'url', url }
 *   { kind: 'open_path', path }
 *   { kind: 'install_update' }
 *   { kind: 'dismiss' }
 *
 * @param {{type?: string, data?: string}|null} action
 * @returns {object}
 */
export function resolveBannerAction(action) {
    if (!action) return { kind: 'dismiss' };
    switch (action.type) {
        case 'settings': {
            // `data` is either a bare section id ('updates') or
            // `<section>:<subsection>` ('updates:changelog'). The
            // subsection is forwarded so settings can scroll to a specific
            // element after switching sections.
            const [section, subSection] = (action.data || 'updates').split(':');
            const out = { kind: 'settings', section: section || 'updates' };
            if (subSection) out.subSection = subSection;
            return out;
        }
        case 'url':
            return { kind: 'url', url: action.data || '' };
        case 'crash_log':
            return { kind: 'open_path', path: action.data || '' };
        case 'update_install':
            return { kind: 'install_update' };
        default:
            return { kind: 'dismiss' };
    }
}

export class BannerController {
    /**
     * @param {object} opts
     * @param {(cmd: string, args?: object) => Promise<any>} opts.invoke
     * @param {() => void} opts.resizeWindow
     * @param {() => void} opts.resetUI  reset the floating UI (dismiss path)
     * @param {() => boolean} opts.isWaitingForResponse
     * @param {object} opts.windowManager  exposes `userSetHeight`
     */
    constructor({ invoke, resizeWindow, resetUI, isWaitingForResponse, windowManager }) {
        this.invoke = invoke;
        this._resizeWindow = resizeWindow;
        this._resetUI = resetUI;
        this._isWaitingForResponse = isWaitingForResponse;
        this.windowManager = windowManager;
        this.visible = false;
        this._action = null;
    }

    /**
     * Show a banner at the top of the content area.
     * @param {string} icon - Emoji or text icon
     * @param {string} html - Banner message (supports HTML for keycaps etc.)
     * @param {string} actionLabel - Text for the action hint
     * @param {string} actionType - 'settings', 'url', 'crash_log', 'update_install', or 'dismiss'
     * @param {string} actionData - Section name, URL, file path, or ignored
     */
    show(icon, html, actionLabel, actionType, actionData) {
        this.visible = true;
        this._action = { type: actionType, data: actionData };
        const banner = document.getElementById('floatingBanner');
        const iconEl = document.getElementById('bannerIcon');
        const textEl = document.getElementById('bannerText');
        const actionEl = document.getElementById('bannerAction');
        const contentArea = document.getElementById('contentArea');
        if (!banner) return;
        if (iconEl) iconEl.textContent = icon || '';
        if (textEl) textEl.innerHTML = html || '';
        if (actionEl) actionEl.textContent = actionLabel || '';
        banner.onclick = () => this.handleClick();
        banner.style.display = 'flex';
        // Ensure the content area is visible so the banner shows
        if (contentArea) {
            contentArea.classList.add('visible');
            // Banner-only mode toggles content-area to overflow:visible
            // so the scrollbar doesn't show for a tiny banner. But that
            // also flips the flex item's min-height from 0 to auto —
            // i.e. it can no longer shrink below its content. If there
            // IS substantial content in here (a streamed response, an
            // image, etc.), enabling banner-only means the content area
            // refuses to shrink, the bubble can't fit within the OS
            // window's max height, and the input gets pushed past the
            // bottom edge. So only switch to banner-only when banner is
            // truly the sole occupant.
            const responseText = document.getElementById('responseText');
            const isEmpty = !responseText?.textContent.trim();
            if (isEmpty) {
                contentArea.classList.add('banner-only');
            } else {
                contentArea.classList.remove('banner-only');
            }
        }
        // Resize the window to fit the banner after DOM updates
        requestAnimationFrame(() => this._resizeWindow());
    }

    handleClick() {
        const intent = resolveBannerAction(this._action);
        this.dismiss();
        switch (intent.kind) {
            case 'settings': {
                const args = { section: intent.section };
                if (intent.subSection) args.subSection = intent.subSection;
                this.invoke('open_settings_window', args).catch(() => {});
                break;
            }
            case 'url':
                this.invoke('open_url', { url: intent.url }).catch(() => {});
                break;
            case 'open_path':
                // Open the crash report file in the OS default editor —
                // text editors handle .log fine on every platform we ship
                // to. Fall back to opening the logs folder if the path is
                // missing for any reason.
                this.invoke('open_path', { path: intent.path }).catch(() => {});
                break;
            case 'install_update':
                // Same flow as the "Install Now" button in settings.
                // Backend produces a classified, user-readable string;
                // formatError unwraps the AppError shape so we don't show
                // "[object Object]" when the rejection is a serialised
                // struct (which it is over the Tauri invoke boundary).
                this.show('⬇️', t('floating.banner.installing_update'), '', 'dismiss', '');
                this.invoke('download_and_install_update').catch((e) => {
                    this.show(
                        '❌',
                        formatError(e),
                        t('floating.banner.action.dismiss'),
                        'dismiss',
                        ''
                    );
                });
                break;
            default:
                // 'dismiss' — reset the UI and refocus input
                this._resetUI();
                this.windowManager.userSetHeight = null;
                this._resizeWindow();
                break;
        }
    }

    dismiss() {
        if (!this.visible) return;
        this.visible = false;
        const banner = document.getElementById('floatingBanner');
        if (banner) banner.style.display = 'none';
        // Drop banner-only mode whether we're collapsing or about to
        // receive a real response; subsequent content needs its
        // scrollbar back.
        document.getElementById('contentArea')?.classList.remove('banner-only');
        // If the banner was the only content, collapse the content area
        const responseText = document.getElementById('responseText');
        if (!this._isWaitingForResponse() && !responseText?.textContent.trim()) {
            document.getElementById('contentArea')?.classList.remove('visible');
            document.getElementById('expandBtn')?.classList.remove('visible');
            this.windowManager.userSetHeight = null;
            this._resizeWindow();
        }
    }

    /**
     * Show a "Kage has been updated!" celebration banner once, after a
     * clean post-update relaunch. Returns true if a banner was shown (the
     * caller suppresses blur-hide for a beat so it isn't stolen by a late
     * window paint).
     *
     * @returns {Promise<boolean>}
     */
    async checkForUpdateBanner() {
        try {
            const wasUpdated = await this.invoke('was_just_updated');
            if (wasUpdated) {
                this.show(
                    '🎉',
                    t('floating.banner.update_installed'),
                    t('floating.banner.action.view_changelog'),
                    'settings',
                    // `<section>:<subsection>` — resolveBannerAction splits
                    // on the colon and forwards both to open_settings_window
                    // so the user lands on the changelog block, not just the
                    // Updates page.
                    'updates:changelog'
                );
                // Clear the flag so it only shows once
                this.invoke('clear_update_flag').catch(() => {});
                return true;
            }
        } catch (e) {
            console.log('Update check failed:', e);
        }
        return false;
    }

    /**
     * Show a "Kage crashed last session" banner if the previous run left a
     * crash report and the user hasn't acknowledged it yet. Only fires once
     * per crash — `dismiss_recent_crash` stamps the timestamp so subsequent
     * launches stay quiet.
     *
     * Held off until after `checkForUpdateBanner` so the celebration banner
     * from a clean post-update relaunch wins the priority fight; in steady
     * state only one of the two ever has anything to say.
     */
    async checkForCrashBanner() {
        try {
            const crash = await this.invoke('get_recent_crash');
            if (!crash) return;
            // Don't stomp on a banner that's already visible (e.g. "Kage
            // has been updated!"). The next launch will check again, and
            // this crash has already been recorded — the user will see the
            // banner the next time they're actually free to read it.
            if (this.visible) return;

            const msg = crash.panic_message
                ? t('floating.banner.crash_with_message', { message: crash.panic_message })
                : t('floating.banner.crash_generic');
            this.show('💥', msg, t('floating.banner.action.view_log'), 'crash_log', crash.log_path);
            // Mark seen now — we've shown the user once. If they ignore the
            // banner we don't re-show; "View log" / any dismiss completes
            // the lifecycle either way. Failure to persist is non-fatal
            // (worst case the dialog reappears next launch).
            this.invoke('dismiss_recent_crash', { timestamp: crash.timestamp }).catch(() => {});
        } catch (e) {
            console.log('Crash banner check failed:', e);
        }
    }
}

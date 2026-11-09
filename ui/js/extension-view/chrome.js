/**
 * The view page's own UI (docs/design/EXTENSION_VIEWS.md §8.2, §18.2,
 * §18.3): the host bar, the status banner, and the loading and "can't
 * open" panels over the stage.
 *
 * All of it is drawn by Kage, outside the view's iframe, so nothing the
 * extension draws can cover it. Every extension-supplied string (its name,
 * the view's title, a subtitle) goes in through `textContent`, never as
 * HTML. Binds to the markup in `ui/extension-view.html`.
 */

/** Buttons the panels and the banner can show, in display order. */
export const PANEL_ACTIONS = Object.freeze(['approve', 'reload', 'settings', 'close']);

const ACTION_LABEL_KEYS = Object.freeze({
    approve: 'ext_view.state.approve',
    reload: 'ext_view.bar.reload',
    settings: 'ext_view.bar.settings',
    close: 'ext_view.bar.close',
});

/** How long "Updated to 1.2.0." stays in the bar. */
export const NOTICE_MS = 3_000;

/**
 * The OS window title. It always ends with the extension's name; an
 * extension only adds a subtitle in front (§17.2).
 *
 * @param {(key: string, vars?: object) => string} t
 * @param {{ view: string, extension: string, subtitle?: string|null }} parts
 */
export function windowTitle(t, { view, extension, subtitle }) {
    return subtitle
        ? t('ext_view.window_title_sub', { subtitle, view, extension })
        : t('ext_view.window_title', { view, extension });
}

/**
 * @param {object} opts
 * @param {Document} [opts.doc]
 * @param {(key: string, vars?: object) => string} opts.t
 * @param {(action: string) => void} opts.onAction - a host button was used
 *   (`reload`, `settings`, `close` or `approve`)
 */
export function createChrome({ doc = document, t, onAction }) {
    const q = (sel) => doc.querySelector(sel);
    const el = {
        icon: q('.kv-bar-icon'),
        ext: q('.kv-bar-ext'),
        sep: q('.kv-bar-sep'),
        view: q('.kv-bar-view'),
        sub: q('.kv-bar-sub'),
        badge: q('.kv-badge'),
        notice: q('.kv-bar-notice'),
        banner: q('.kv-banner'),
        bannerText: q('.kv-banner-text'),
        bannerActions: q('.kv-banner-actions'),
        frame: q('.kv-frame'),
        loading: q('.kv-loading'),
        loadingText: q('.kv-loading-text'),
        state: q('.kv-state'),
        stateText: q('.kv-state-text'),
        stateActions: q('.kv-state-actions'),
    };
    let noticeTimer = null;

    // One delegated handler for the bar, the banner and the panels. Buttons
    // carry `data-kv-action`; the handler never reads anything else.
    doc.body.addEventListener('click', (e) => {
        const btn = e.target?.closest?.('[data-kv-action]');
        if (!btn || !doc.body.contains(btn)) return;
        const action = btn.getAttribute('data-kv-action');
        if (!PANEL_ACTIONS.includes(action)) return;
        e.preventDefault();
        onAction(action);
    });

    function renderActions(container, actions) {
        container.replaceChildren();
        for (const action of PANEL_ACTIONS) {
            if (!actions?.includes(action)) continue;
            const btn = doc.createElement('button');
            btn.type = 'button';
            btn.className = action === 'approve' ? 'kv-btn kv-btn-primary' : 'kv-btn';
            btn.setAttribute('data-kv-action', action);
            btn.textContent = t(ACTION_LABEL_KEYS[action]);
            container.appendChild(btn);
        }
    }

    return {
        /** Where the view sandbox mounts its iframe. */
        stage: el.frame,

        /** The view's iframe, once the sandbox has created it. */
        get frame() {
            return el.frame.querySelector('iframe');
        },

        /**
         * The start side of the bar: icon, extension name, "·", view title.
         * @param {{ icon?: string, extensionName: string, viewTitle: string }} identity
         */
        setIdentity({ icon, extensionName, viewTitle }) {
            el.icon.textContent = icon || '';
            el.icon.hidden = !icon;
            el.ext.textContent = extensionName || '';
            el.view.textContent = viewTitle || '';
            el.sep.hidden = !extensionName || !viewTitle;
            el.badge.title = t('ext_view.badge_tip', { extension: extensionName || '' });
        },

        /** The view's subtitle (`context.view.setTitle`), or none. */
        setSubtitle(text) {
            el.sub.textContent = text || '';
            el.sub.hidden = !text;
        },

        get subtitle() {
            return el.sub.hidden ? null : el.sub.textContent;
        },

        showLoading(viewTitle) {
            el.loadingText.textContent = t('ext_view.loading', { view: viewTitle || '' });
            el.loading.hidden = false;
            el.loading.setAttribute('aria-busy', 'true');
            el.state.hidden = true;
        },

        hideLoading() {
            el.loading.hidden = true;
            el.loading.removeAttribute('aria-busy');
        },

        /**
         * The "can't open" panel and its variants (§18.3).
         * @param {{ message: string, actions?: string[] }} state
         */
        showState({ message, actions = [] }) {
            el.stateText.textContent = message || '';
            renderActions(el.stateActions, actions);
            el.loading.hidden = true;
            el.loading.removeAttribute('aria-busy');
            el.state.hidden = false;
        },

        hideState() {
            el.state.hidden = true;
            el.stateActions.replaceChildren();
        },

        get stateVisible() {
            return !el.state.hidden;
        },

        /** The banner under the bar ("isn't responding"). */
        showBanner({ message, actions = [] }) {
            el.bannerText.textContent = message || '';
            renderActions(el.bannerActions, actions);
            el.banner.hidden = false;
        },

        hideBanner() {
            el.banner.hidden = true;
            el.bannerActions.replaceChildren();
        },

        get bannerVisible() {
            return !el.banner.hidden;
        },

        /** A short note in the bar ("Updated to 1.2.0."). */
        flashNotice(text, ms = NOTICE_MS) {
            if (noticeTimer) clearTimeout(noticeTimer);
            el.notice.textContent = text || '';
            el.notice.hidden = !text;
            noticeTimer = setTimeout(() => {
                noticeTimer = null;
                el.notice.hidden = true;
                el.notice.textContent = '';
            }, ms);
        },
    };
}

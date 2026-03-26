/**
 * Global link click interceptor.
 *
 * - http/https links → open in default browser via Tauri
 * - kage: protocol → route to internal actions (store, settings, etc.)
 * - All other clicks on <a> tags → prevent default navigation
 *
 * Usage: import { initLinkHandler } from './link-handler.js';
 *        initLinkHandler(invoke);
 */

let _invoke = null;

/**
 * kage: protocol routes.
 * Format: kage:<action>[/<param>]
 *
 * Supported routes:
 *   kage:store              → open extension store
 *   kage:store/themes       → open store on themes tab
 *   kage:store/extensions   → open store on extensions tab
 *   kage:store/commands     → open store on commands tab
 *   kage:settings           → open settings window
 *   kage:settings/<section> → open settings on a specific section
 */
async function handleKageProtocol(path) {
    if (!_invoke) return;

    const parts = path.split('/').filter(Boolean);
    const action = parts[0];
    const param = parts[1] || null;

    switch (action) {
        case 'store':
            await _invoke('open_store_window', { tab: param || null });
            break;
        case 'settings':
            await _invoke('open_settings_window', { section: param || null });
            break;
        default:
            console.warn(`Unknown kage: route "${path}"`);
    }
}

/**
 * Initialize the global link click handler.
 * Call once per window on startup.
 */
export function initLinkHandler(invoke) {
    _invoke = invoke;

    document.addEventListener('click', (e) => {
        const anchor = e.target.closest('a');
        if (!anchor) return;

        const href = anchor.getAttribute('href');
        if (!href || href === '#') return;

        // kage: protocol — internal deep links
        if (href.startsWith('kage:')) {
            e.preventDefault();
            e.stopPropagation();
            const path = href.slice('kage:'.length);
            handleKageProtocol(path).catch(err =>
                console.warn('kage: link error:', err)
            );
            return;
        }

        // External URLs — open in default browser
        if (href.startsWith('http://') || href.startsWith('https://')) {
            e.preventDefault();
            e.stopPropagation();
            invoke('open_url', { url: href }).catch(err =>
                console.warn('Failed to open URL:', err)
            );
            return;
        }

        // mailto: links — let the OS handle them
        if (href.startsWith('mailto:')) {
            e.preventDefault();
            e.stopPropagation();
            invoke('open_url', { url: href }).catch(() => {});
            return;
        }

        // Anything else — prevent navigation away from the app
        e.preventDefault();
    });
}

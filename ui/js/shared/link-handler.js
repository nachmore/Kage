/**
 * Global link click interceptor.
 *
 * - http/https links → open in default browser via Tauri
 * - assistant: protocol → route to internal actions (store, settings, etc.)
 * - All other clicks on <a> tags → prevent default navigation
 *
 * Usage: import { initLinkHandler } from './link-handler.js';
 *        initLinkHandler(invoke);
 */

let _invoke = null;

/**
 * assistant: protocol routes.
 * Format: assistant:<action>[/<param>]
 *
 * Supported routes:
 *   assistant:store              → open extension store
 *   assistant:store/themes       → open store on themes tab
 *   assistant:store/extensions   → open store on extensions tab
 *   assistant:store/commands     → open store on commands tab
 *   assistant:settings           → open settings window
 *   assistant:settings/<section> → open settings on a specific section
 */
async function handleAssistantProtocol(path) {
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
            console.warn(`Unknown assistant: route "${path}"`);
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

        // assistant: protocol — internal deep links
        if (href.startsWith('assistant:')) {
            e.preventDefault();
            e.stopPropagation();
            const path = href.slice('assistant:'.length);
            handleAssistantProtocol(path).catch(err =>
                console.warn('assistant: link error:', err)
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

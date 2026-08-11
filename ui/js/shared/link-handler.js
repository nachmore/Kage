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
 * Guard against double-registration. The click listener is delegated on
 * `document`, so registering it twice makes every link open twice (each
 * listener independently calls `open_url` — `stopPropagation` only stops
 * bubbling between nodes, not sibling listeners on the same node). If
 * `initLinkHandler` is ever called more than once in a window (double module
 * eval, a re-init on config change, a future refactor), we refresh `_invoke`
 * but do NOT add a second listener.
 */
let _listenerInstalled = false;

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
 * Neutralize navigable anchors inside `root` so WebView2 can't open them
 * itself. For every `<a>` with an http(s)/mailto/kage: href, move the URL to
 * `data-href` and set `href="#"`. The delegated click handler reads `data-href`
 * and opens the URL exactly once via `open_url`.
 *
 * Why this is needed: a real `href` makes WebView2 perform a top-level
 * navigation on click, which Tauri hands to the OS default browser — a SECOND
 * open, in parallel with our handler's `open_url`, and one that never hits our
 * command (so it produced no log while doubling every link). `preventDefault()`
 * in the click handler does not reliably cancel that native navigation. Killing
 * the navigable href removes the second open at the source. Idempotent: an
 * anchor already carrying `data-href` is skipped.
 *
 * Call after rendering agent markdown (streaming and final).
 *
 * @param {ParentNode} root - container whose descendant <a> tags to neutralize
 */
export function neutralizeLinks(root) {
    if (!root || typeof root.querySelectorAll !== 'function') return;
    for (const a of root.querySelectorAll('a[href]')) {
        if (a.hasAttribute('data-href')) continue; // already neutralized
        const href = a.getAttribute('href');
        if (!href || href === '#') continue;
        // Only intercept schemes we actually handle; leave in-page anchors
        // (#section) and anything unrecognised alone.
        if (
            href.startsWith('http://') ||
            href.startsWith('https://') ||
            href.startsWith('mailto:') ||
            href.startsWith('kage:')
        ) {
            a.setAttribute('data-href', href);
            a.setAttribute('href', '#');
        }
    }
}

/**
 * Initialize the global link click handler.
 * Call once per window on startup.
 */
export function initLinkHandler(invoke) {
    _invoke = invoke;

    // Idempotent: only ever install one delegated listener per document.
    if (_listenerInstalled) {
        console.warn('[link-handler] initLinkHandler called again — reusing existing listener');
        return;
    }
    _listenerInstalled = true;

    document.addEventListener('click', (e) => {
        const anchor = e.target.closest('a');
        if (!anchor) return;

        // Prefer data-href: neutralizeLinks() moves external URLs there and
        // sets href="#" so WebView2 has nothing to natively navigate to. A
        // real href would make WebView2 open the URL ITSELF (a top-level
        // navigation the OS then hands to the default browser) IN ADDITION to
        // our open_url call — that was the "opens twice" bug, and it bypassed
        // open_url entirely (no log), which is why no open_url-side fix helped.
        const href = anchor.getAttribute('data-href') || anchor.getAttribute('href');
        if (!href || href === '#') return;

        // kage: protocol — internal deep links
        if (href.startsWith('kage:')) {
            e.preventDefault();
            e.stopPropagation();
            const path = href.slice('kage:'.length);
            handleKageProtocol(path).catch((err) => console.warn('kage: link error:', err));
            return;
        }

        // External URLs — open in default browser. Use the module-level
        // `_invoke` (not the closure param) so a re-init refreshes the
        // reference — consistent with the kage: branch above.
        if (href.startsWith('http://') || href.startsWith('https://')) {
            e.preventDefault();
            e.stopPropagation();
            console.log(`[link-handler] open_url: ${href}`);
            _invoke?.('open_url', { url: href }).catch((err) =>
                console.warn('Failed to open URL:', err)
            );
            return;
        }

        // mailto: links — let the OS handle them
        if (href.startsWith('mailto:')) {
            e.preventDefault();
            e.stopPropagation();
            _invoke?.('open_url', { url: href }).catch(() => {});
            return;
        }

        // Anything else — prevent navigation away from the app
        e.preventDefault();
    });
}

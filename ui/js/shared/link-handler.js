/**
 * Global link click interceptor — installed in EVERY Kage window.
 *
 * - http/https/mailto links → open in the default browser / mail client via
 *   the `open_url` command (left- and middle-click)
 * - kage: protocol → route to internal actions (store, settings, etc.)
 * - All other clicks on <a> tags → prevent default navigation
 * - Exception: a script-built `<a download href="blob:…">` (a settings
 *   "Export" host effect) is left alone so the download goes through.
 *
 * A window without this handler navigates ITSELF to any plain
 * <a href="https://…"> the user clicks. The Rust navigation guard
 * (src/navigation_guard.rs) cancels that as a backstop, but on macOS/Linux it
 * can't tell a user's click from an extension iframe's redirect, so it only
 * cancels — opening the browser is this module's job.
 *
 * The listeners sit on `window` in the CAPTURE phase — the first stop of
 * every click — so no page handler can hide a click from them. A bubbling
 * handler missed every click under an ancestor that calls
 * stopPropagation() (the welcome wizard's privacy label, the diagram
 * lightbox): nothing cancelled it, and the link navigated the window in
 * place.
 *
 * Because this runs before every other handler, a first-party handler that
 * handles an anchor click itself must leave nothing here to open: give the
 * anchor `href="#"` (or no href) and keep the URL elsewhere (source chips
 * use `data-url`), or mark it `data-action` (settings dispatcher) /
 * `data-ext-action` (extension actions). Those clicks are still cancelled,
 * just not opened.
 *
 * Usage: import { initLinkHandler } from './link-handler.js';
 *        initLinkHandler(invoke);
 */

let _invoke = null;

const XLINK_NS = 'http://www.w3.org/1999/xlink';

/**
 * Guard against double-registration. The click listener is delegated on
 * `window`, so registering it twice makes every link open twice (each
 * listener independently calls `open_url`). If `initLinkHandler` is ever
 * called more than once in a window (double module eval, a re-init on config
 * change, a future refactor), we refresh `_invoke` but do NOT add a second
 * listener.
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
 * Why this is needed: links with a live `href` were opened a SECOND time, in
 * parallel with our handler's `open_url`, by a path that never hit our command
 * (no log while doubling every link) — tauri-plugin-shell's injected
 * target=_blank opener, which the click handler now also defuses. Without a
 * navigable href there is nothing for the engine or any other listener to
 * follow if the click handler isn't there; the Rust navigation guard is the
 * last line. Idempotent: an anchor already carrying `data-href` is skipped.
 *
 * Call after rendering agent markdown (streaming and final) and after
 * injecting sanitized extension HTML (formatters, settings info blocks).
 *
 * @param {ParentNode} root - container whose descendant <a> tags to neutralize
 */
export function neutralizeLinks(root) {
    if (!root || typeof root.querySelectorAll !== 'function') return;
    for (const a of root.querySelectorAll('a[href]')) {
        if (a.hasAttribute('data-href')) continue; // already neutralized
        const href = a.getAttribute('href');
        // Only intercept schemes we actually handle; leave in-page anchors
        // (#section) and anything unrecognised alone.
        if (classifyLinkHref(href) !== 'none') {
            a.setAttribute('data-href', href);
            a.setAttribute('href', '#');
            // The sanitizer forces target=_blank on http(s) links. Left on a
            // neutralized anchor, tauri-plugin-shell's injected target=_blank
            // opener hands the resolved "#" to the OS browser as
            // http://tauri.localhost/# — alongside our own open_url. (The
            // click handler also strips target at capture time; this keeps
            // the rendered DOM honest too.)
            a.removeAttribute('target');
        }
    }
}

/**
 * What a click on an anchor with this href should do:
 *   'kage'     — internal deep link (kage:store, kage:settings/<section>)
 *   'external' — hand to the OS default handler via open_url (http, https,
 *                mailto). The same set the Rust navigation guard forwards.
 *   'none'     — nothing; the click is just cancelled (javascript:, file:,
 *                in-page "#…", custom app schemes, empty).
 *
 * Case-insensitive and tolerant of leading whitespace, like the engine's own
 * URL parser — `HTTPS://…` must not fall through to 'none' and get dropped.
 *
 * @param {string|null|undefined} href
 * @returns {'kage'|'external'|'none'}
 */
export function classifyLinkHref(href) {
    const h = String(href ?? '').trimStart();
    if (!h || h.startsWith('#')) return 'none';
    const lower = h.toLowerCase();
    if (lower.startsWith('kage:')) return 'kage';
    if (
        lower.startsWith('http://') ||
        lower.startsWith('https://') ||
        lower.startsWith('mailto:')
    ) {
        return 'external';
    }
    return 'none';
}

/**
 * Prefer data-href: neutralizeLinks() moves external URLs there and sets
 * href="#" so the webview has nothing to natively navigate to. SVG anchors
 * carry a namespaced xlink:href that getAttribute('href') doesn't see.
 */
function hrefOf(anchor) {
    return (
        anchor.getAttribute('data-href') ||
        anchor.getAttribute('href') ||
        anchor.getAttributeNS(XLINK_NS, 'href')
    );
}

/**
 * A script-built download link: `<a download href="blob:…">` clicked by
 * `settings-host-effects` for an extension's "Export". Its activation saves a
 * file and never leaves the page (the Rust guard treats blob: as inert), so
 * it must not be cancelled — that silently dropped the export. Only blob:
 * qualifies: blob URLs are minted by this document's own script, whereas a
 * data: URL is plain markup anyone who can inject HTML could author.
 */
function isBlobDownload(anchor) {
    if (!anchor.hasAttribute('download')) return false;
    const href = String(anchor.getAttribute('href') ?? '').trimStart();
    return href.toLowerCase().startsWith('blob:');
}

/**
 * A first-party handler owns this anchor's click and does its own thing:
 * the settings action dispatcher (`data-action`, which also cancels anchor
 * clicks) or an extension action (`data-ext-action`, wired by the extension
 * manager). Opening its URL as well would double up.
 */
function ownedByHandler(anchor) {
    return anchor.hasAttribute('data-action') || anchor.closest('[data-ext-action]') !== null;
}

function onActivate(e) {
    const anchor = e.target?.closest?.('a');
    if (!anchor) return;
    if (isBlobDownload(anchor)) return;

    // Drop `target`. tauri-plugin-shell injects a `body` click listener into
    // every window that opens `target="_blank"` http(s)/mailto/tel links
    // through `plugin:shell|open` — in addition to our open_url below, so
    // those links opened twice (and a neutralized `href="#"` one opened
    // http://tauri.localhost/#). Without `target` its predicate never matches
    // and this module is the only opener. Kage windows never want an
    // engine-created popup anyway — the Rust navigation guard denies them.
    anchor.removeAttribute('target');

    // Cancel native activation for EVERY anchor up front — the only URLs we
    // follow are the ones we explicitly open below. A plain
    // <a href="https://…"> left alone navigates the whole window to the
    // remote page (an extension's setup link turned the Settings window into
    // developer.spotify.com that way), and an SVG <a xlink:href="javascript:…">
    // (e.g. from a Graphviz diagram) would run in this privileged webview.
    // Done here in capture, a later stopPropagation() can't undo it.
    const claimed = e.defaultPrevented; // by an earlier window-capture listener
    e.preventDefault();
    if (claimed || ownedByHandler(anchor)) return;

    const href = hrefOf(anchor);
    switch (classifyLinkHref(href)) {
        case 'kage': {
            const path = href.trimStart().slice('kage:'.length);
            handleKageProtocol(path).catch((err) => console.warn('kage: link error:', err));
            return;
        }
        case 'external': {
            // Module-level `_invoke` (not a closure param) so a re-init
            // refreshes the reference.
            const url = href.trim();
            console.log(`[link-handler] open_url: ${url}`);
            _invoke?.('open_url', { url }).catch((err) => console.warn('Failed to open URL:', err));
            return;
        }
        default:
            // Already prevented above, so no navigation away from the app.
            return;
    }
}

function onAuxClick(e) {
    // A middle-click asks the engine for a new window, which the Rust guard
    // denies — without this, middle-clicking a link did nothing at all.
    // Right-click (button 2) keeps its context menu.
    if (e.button !== 1) return;
    onActivate(e);
}

/**
 * Initialize the global link click handler.
 * Call once per window on startup — every Kage window does; see
 * docs/SECURITY_MODEL.md § "Links and navigation".
 */
export function initLinkHandler(invoke) {
    _invoke = invoke;

    // Idempotent: only ever install one delegated listener per window.
    if (_listenerInstalled) {
        console.warn('[link-handler] initLinkHandler called again — reusing existing listener');
        return;
    }
    _listenerInstalled = true;

    // Capture phase on window: ahead of every listener in the page, so an
    // ancestor's stopPropagation() can't keep a click from being cancelled.
    window.addEventListener('click', onActivate, true);
    window.addEventListener('auxclick', onAuxClick, true);
}

/**
 * Remove the listeners again. Test-only: jsdom keeps one `window` across a
 * test file, and a stale handler from an earlier module instance would claim
 * every click (preventDefault) before the instance under test sees it.
 */
export function disposeLinkHandler() {
    if (!_listenerInstalled) return;
    window.removeEventListener('click', onActivate, true);
    window.removeEventListener('auxclick', onAuxClick, true);
    _listenerInstalled = false;
    _invoke = null;
}

// Shared renderer for extension toolbar buttons, used by both the chat
// and floating windows. The two windows differ only in their container
// element, CSS class, icon sanitizer, click-context shape, and host-effect
// handler — everything else (clear-then-rebuild, sanitized icon, click
// wiring with error isolation) was copy-pasted. This module centralizes the
// rendering; each window passes its specifics in.

/**
 * Render `buttons` (from `ExtensionManager.getToolbarButtons()`) into
 * `container`, clearing any previously-rendered extension buttons first.
 *
 * @param {object} opts
 * @param {HTMLElement|null} opts.container  element to render into
 * @param {Array} opts.buttons               toolbar button descriptors
 * @param {string} opts.buttonClass          per-window button CSS class
 *   (combined with the shared `ext-toolbar-btn` marker class)
 * @param {(iconStr: string) => Node} opts.sanitizeIcon  returns a DOM node
 *   for the (untrusted) icon string, sanitized via the extension sanitizer
 * @param {(btn: object) => object} opts.buildContext  builds the context
 *   object passed to `btn.onClick`
 * @param {(host: object, btn: object) => void} opts.onHostEffect  applies a
 *   host effect returned from `onClick`
 */
export function renderToolbarButtons(opts) {
    const { container, buttons, buttonClass, sanitizeIcon, buildContext, onHostEffect } = opts;
    if (!container) return;

    // Clear previously-rendered extension buttons. Some containers hold
    // only extension buttons (floating) and some are shared with native
    // controls (chat) — removing by the shared marker class is correct for
    // both.
    container.querySelectorAll('.ext-toolbar-btn').forEach((el) => el.remove());

    for (const btn of buttons) {
        const el = document.createElement('button');
        el.className = `${buttonClass} ext-toolbar-btn`;
        el.title = btn.tooltip || btn.id;
        // Icons are sanitized through the `icon` mode of the extension
        // sanitizer: SVG markup renders as SVG; emoji / plain text passes
        // through as a text node; anything else (anchors, images, scripts,
        // on* handlers, javascript: URLs) is stripped. This keeps the
        // sandbox boundary intact while letting extensions ship sharp icons.
        const iconStr = typeof btn.icon === 'string' && btn.icon ? btn.icon : '🔧';
        el.appendChild(sanitizeIcon(iconStr));
        el.addEventListener('click', async () => {
            try {
                const ctx = buildContext(btn);
                const out = await btn.onClick(ctx);
                if (out?.host) onHostEffect(out.host, btn);
            } catch (e) {
                console.warn(`Extension toolbar button error (${btn.extensionId}):`, e);
            }
        });
        container.appendChild(el);
    }
}

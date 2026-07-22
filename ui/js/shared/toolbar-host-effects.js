// Shared applier for host effects returned by extension toolbar-button
// clicks (see `renderToolbarButtons` in extension-toolbar.js). The chat
// and floating windows differ only in which input element receives
// set/append and how (or whether) ephemeral messages render — callers
// pass those specifics in. Contract mirrors the settings-page effects
// (settings-host-effects.js), narrowed to what makes sense from a
// toolbar click.

/**
 * Apply a toolbar host effect.
 *
 * @param {object} host  effect object from the extension ({ type, value, … })
 * @param {object} opts
 * @param {HTMLInputElement|HTMLTextAreaElement|null} opts.input  the window's
 *   chat input for set_chat_input / append_chat_input
 * @param {(host: object) => void} [opts.onEphemeralMessage]  render a
 *   sanitized ephemeral bubble; omit in windows without a messages area
 *   (the effect is logged and dropped so extensions can tell unsupported
 *   contexts from silent failure)
 * @param {string} [opts.logTag]  window tag for console messages
 */
export function runToolbarHostEffect(host, opts) {
    if (!host || typeof host !== 'object') return;
    const { input, onEphemeralMessage, logTag = 'Toolbar' } = opts || {};
    switch (host.type) {
        case 'set_chat_input': {
            const v = String(host.value ?? '');
            if (input) {
                input.value = v;
                input.focus();
                // Trigger input event so autogrow + suggestions update.
                input.dispatchEvent(new Event('input'));
            }
            break;
        }
        case 'append_chat_input': {
            const v = String(host.value ?? '');
            if (input) {
                const cur = input.value || '';
                const sep = cur && !cur.endsWith(' ') ? ' ' : '';
                input.value = cur + sep + v;
                input.focus();
                input.dispatchEvent(new Event('input'));
            }
            break;
        }
        case 'show_ephemeral_message': {
            if (onEphemeralMessage) {
                onEphemeralMessage(host);
            } else {
                console.info(
                    `[${logTag}] Ignoring show_ephemeral_message host effect — only supported in chat window`
                );
            }
            break;
        }
        default:
            console.warn(`[${logTag}] Unknown toolbar host effect:`, host.type);
            break;
    }
}

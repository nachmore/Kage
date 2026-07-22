// Shared base for the execution context handed to
// `executeShortcutCommand` / `executeResultShared` (result-executor.js).
// The chat and floating windows build the same core — invoke, appWindow,
// extensionManager, a clipboard-writing onCopy, and an onReplaceInput
// that fills the window's input and re-fires its `input` event — then
// layer window-specific callbacks (onPrompt, onDisplay, timers, …) on
// top. Only the shared core lives here; pass everything else through
// `extra`.

/**
 * @param {object} opts
 * @param {Function} opts.invoke
 * @param {object} opts.appWindow
 * @param {object|null} opts.extensionManager
 * @param {HTMLInputElement|HTMLTextAreaElement|null} opts.input  target of
 *   the default onReplaceInput (used by keyword completion hints to fill
 *   the input with the full trigger and re-run search)
 * @param {object} [opts.extra]  window-specific ctx members; may override
 *   any default (e.g. a custom onReplaceInput)
 */
export function buildExecCtx({ invoke, appWindow, extensionManager, input, extra }) {
    return {
        invoke,
        appWindow,
        extensionManager,
        onCopy: async (text) => {
            try {
                await navigator.clipboard.writeText(text);
            } catch {}
        },
        onReplaceInput: (text) => {
            if (!input) return;
            input.value = text;
            input.dispatchEvent(new Event('input', { bubbles: true }));
        },
        ...extra,
    };
}

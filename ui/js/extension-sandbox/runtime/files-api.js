/**
 * `context.files` for extensions whose window offers `files.pick_text`
 * (they hold `file_text`): ask the user to pick a file and get its text
 * (docs/design/EXTENSION_FILE_TEXT.md §3.4). A thin wrapper over the
 * `pick_file_text`, `cancel_file_text` and `file_text_support` commands.
 * Self-contained (no imports): the host inlines this file into the
 * sandbox runtime (RUNTIME_SOURCE_PATHS in extension-sandbox-host.js).
 *
 * @param {{ invoke: (command: string, args?: object) => Promise<any> }} deps
 */
export function createFilesApi({ invoke }) {
    return Object.freeze({
        /**
         * What this Kage reads: `{ formats: [{ id, extensions }],
         * max_input_bytes, max_chars, default_max_chars }`.
         */
        support() {
            return invoke('file_text_support', {});
        },

        /**
         * Ask the user to pick a file and return its text (`FileText`), or
         * null when they cancelled the dialog, said "Not now", or the read
         * was cancelled (through `signal` or the progress chip's Cancel).
         * Other failures reject with an Error carrying `kind` and `key`.
         *
         * @param {{ formats?: string[], maxChars?: number, signal?: AbortSignal }} [options]
         */
        async pickText(options = {}) {
            const { formats, maxChars, signal } = options || {};
            if (signal?.aborted) return null;
            const args = {};
            if (formats !== undefined) args.formats = formats;
            if (maxChars !== undefined) args.max_chars = maxChars;
            const onAbort = () => {
                invoke('cancel_file_text', {}).catch(() => {});
            };
            const listens = typeof signal?.addEventListener === 'function';
            if (listens) signal.addEventListener('abort', onAbort, { once: true });
            try {
                return (await invoke('pick_file_text', { args })) ?? null;
            } catch (err) {
                if (err?.key === 'errors.file_text.cancelled') return null;
                throw err;
            } finally {
                if (listens) signal.removeEventListener('abort', onAbort);
            }
        },
    });
}

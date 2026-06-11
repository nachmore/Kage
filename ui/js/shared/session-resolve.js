/**
 * Shared helper for fetching a window's pinned ACP session id when the
 * value is about to be handed to a backend agent command.
 *
 * The pattern this replaces, duplicated across inline-assist, slash
 * commands, and the script/automation generators:
 *
 *   const sessionId = await invoke('get_window_session', { label })
 *       .catch(() => null);
 *   await invoke('send_inline_assist', { sessionId, ... });
 *
 * `get_window_session` returns `null` whenever the target window hasn't
 * pinned a session yet (it was never opened this run), and the `.catch`
 * turns a transient lookup failure into the same `null`. The backend
 * agent commands that consume this — send_inline_assist,
 * open_chat_with_message, execute_macro, execute_slash_command — take
 * `Option<String>` and create a real session when they receive null, so
 * passing the nullable value straight through is exactly right. This
 * helper just makes that intent explicit and keeps the swallow-to-null
 * behaviour in one place instead of re-derived at every call site.
 *
 * @param {(cmd: string, args?: object) => Promise<any>} invoke - Tauri invoke fn
 * @param {string} label - window label whose pinned session to read
 * @returns {Promise<string|null>} the session id, or null if unpinned/unavailable
 */
export async function getWindowSessionOrNull(invoke, label) {
    return invoke('get_window_session', { label }).catch(() => null);
}

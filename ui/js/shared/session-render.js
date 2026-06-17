// Pure session-history → render-queue logic, lifted out of chat/app.js so
// the flow that decides which messages to display (and how to label them)
// is testable in isolation.
//
// The chat window's `displaySession` reads a session JSONL into messages
// and feeds them through `buildRenderQueue` to get a flat list of items
// the renderer can map 1:1 to DOM. The non-obvious bits this layer handles:
//
// - Steering messages (`[KAGE_STEERING_IGNORE]` prefix) are kept but tagged
//   `type: 'steering'` and the *next* assistant message is consumed as a
//   `steering_ack` rather than a normal reply. This stops the
//   "ack" / "got it" responses from bloating the timeline.
// - Timestamp injections (`[Current time: ...]`) are metadata, not user
//   text — they're stripped from the displayed prompt.
// - Images are extracted and listed alongside text parts so the renderer
//   can show them as snapshots.
//
// Callers must inject a `sessionImageToDataUrl` adapter so this module
// stays free of Tauri / browser-specific image decoding.

import { sessionImageToDataUrl as _defaultSessionImageToDataUrl } from './attachments.js';

/** Prefix used to identify steering messages that should be hidden in the UI. */
export const STEERING_MSG_PREFIX = '[KAGE_STEERING_IGNORE]';

/**
 * Build metadata for a single message based on the session's per-id
 * timestamp and duration maps. Returns `null` if the message has no
 * recorded end timestamp (we don't fabricate one).
 *
 * For user messages with a known duration, the recorded `endTs` is
 * the *send* time but durations are agent-time, so the user's
 * timestamp is end - duration. Assistant messages keep `endTs` as-is
 * and additionally record `durationSecs` for "thought for Ns" labels.
 */
export function buildMsgMeta(messageId, timestamps, durations, role) {
    if (!messageId) return null;
    const endTs = timestamps[messageId];
    if (!endTs) return null;
    const dur = durations[messageId] || 0;
    if (role === 'user' && dur > 0) {
        const endDate = new Date(endTs);
        return { timestamp: new Date(endDate.getTime() - dur * 1000).toISOString() };
    }
    return { timestamp: endTs, durationSecs: role === 'assistant' ? dur : null };
}

/**
 * Walk a list of session messages and emit a flat queue of render items.
 *
 * Output shape per item:
 *   { type: 'user',          text, snapshots, meta }
 *   { type: 'assistant',     text, meta }
 *   { type: 'steering',      text }
 *   { type: 'steering_ack',  text }
 *
 * `imageToDataUrl` is the adapter used to materialize image content into
 * displayable data URLs. Defaults to the shared `sessionImageToDataUrl`,
 * but tests can pass a stub to avoid btoa/Uint8Array round-trips.
 */
export function buildRenderQueue(
    messages,
    timestamps,
    durations,
    imageToDataUrl = _defaultSessionImageToDataUrl
) {
    const queue = [];
    let skipNextAssistant = false;

    for (const msg of messages) {
        if (msg.kind === 'Prompt') {
            const textParts = [];
            const imageDataUrls = [];
            let isSteering = false;
            for (const item of msg.content) {
                if (item.kind === 'text' && typeof item.data === 'string') {
                    if (item.data.startsWith(STEERING_MSG_PREFIX)) {
                        isSteering = true;
                        textParts.push(item.data.substring(STEERING_MSG_PREFIX.length).trim());
                        continue;
                    }
                    if (item.data.trim().startsWith('[Current time:')) {
                        continue;
                    }
                    textParts.push(item.data);
                } else if (item.kind === 'image') {
                    const dataUrl = imageToDataUrl(item);
                    if (dataUrl) imageDataUrls.push(dataUrl);
                }
            }

            if (isSteering) {
                skipNextAssistant = true;
                queue.push({ type: 'steering', text: textParts.join('\n\n') });
                continue;
            }

            if (textParts.length > 0 || imageDataUrls.length > 0) {
                const text = textParts.join('\n');
                const snapshots =
                    imageDataUrls.length > 0
                        ? imageDataUrls.map((url) => ({ type: 'image', previewUrl: url }))
                        : null;
                queue.push({
                    type: 'user',
                    text,
                    snapshots,
                    meta: buildMsgMeta(msg.message_id, timestamps, durations, 'user'),
                });
            }
        } else if (msg.kind === 'AssistantMessage') {
            if (skipNextAssistant) {
                skipNextAssistant = false;
                const ackText = [];
                for (const item of msg.content) {
                    if (item.kind === 'text' && typeof item.data === 'string' && item.data.trim()) {
                        ackText.push(item.data.trim());
                    }
                }
                if (ackText.length > 0) {
                    queue.push({ type: 'steering_ack', text: ackText.join(' ') });
                }
                continue;
            }
            const textParts = [];
            for (const item of msg.content) {
                if (item.kind === 'text' && typeof item.data === 'string' && item.data.trim()) {
                    textParts.push(item.data);
                }
            }
            if (textParts.length > 0) {
                queue.push({
                    type: 'assistant',
                    text: textParts.join('\n\n'),
                    meta: buildMsgMeta(msg.message_id, timestamps, durations, 'assistant'),
                });
            }
        }
    }
    return queue;
}

/**
 * Format a duration in seconds as "Xs", "XmYs", or "Xm".
 * Used for the "thought for ..." label on assistant messages.
 */
export function formatDuration(totalSecs) {
    const secs = Math.round(totalSecs);
    if (secs < 60) return `${secs}s`;
    const mins = Math.floor(secs / 60);
    const rem = secs % 60;
    return rem > 0 ? `${mins}m${rem}s` : `${mins}m`;
}

/**
 * Order + filter sessions for the chat sidebar. Pure — separated from the
 * DOM-diffing in `ChatApp.renderSessionList` so the ranking rules are
 * unit-testable. Rules:
 *   - The floating window's pinned session (`defaultId`) sorts to the top;
 *     everything else is newest-first by `updated_at`.
 *   - With a search query, match case-insensitively against the title
 *     (defaulting an absent title to "New Chat").
 *   - Without a query, hide steering-only "New Chat" sessions UNLESS the row
 *     is the default session or one of the active/selected ids — otherwise a
 *     freshly-created peer would vanish from the sidebar mid-click.
 *
 * @param {Array<{session_id: string, title?: string, updated_at?: string, created_at?: string}>} sessions
 * @param {object} opts
 * @param {string} [opts.defaultId]  floating window's pinned session id
 * @param {string} [opts.searchQuery]  raw search box value
 * @param {string[]} [opts.keepIds]  ids to keep even if they're "New Chat"
 *   (e.g. the active selection + current ACP session)
 * @returns {Array} the filtered, ordered sessions
 */
export function orderSessionsForSidebar(sessions, opts = {}) {
    const { defaultId, searchQuery = '', keepIds = [] } = opts;
    const query = searchQuery.toLowerCase().trim();

    const sorted = [...sessions].sort((a, b) => {
        const aIsDefault = a.session_id === defaultId;
        const bIsDefault = b.session_id === defaultId;
        if (aIsDefault && !bIsDefault) return -1;
        if (!aIsDefault && bIsDefault) return 1;
        return (b.updated_at || '').localeCompare(a.updated_at || '');
    });

    if (query) {
        return sorted.filter((s) => (s.title || 'New Chat').toLowerCase().includes(query));
    }
    const keep = new Set(keepIds);
    return sorted.filter((s) => {
        const title = s.title || 'New Chat';
        if (title !== 'New Chat') return true;
        return s.session_id === defaultId || keep.has(s.session_id);
    });
}

/**
 * Format a relative date for the session list — "HH:mm" today, "Yesterday",
 * weekday name within a week, or "Mon DD" beyond. Pure given a `now`
 * reference (defaults to `new Date()`).
 */
export function formatRelativeDate(date, now = new Date()) {
    const diff = now - date;
    const days = Math.floor(diff / (1000 * 60 * 60 * 24));

    if (days === 0) {
        return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } else if (days === 1) {
        return 'Yesterday';
    } else if (days < 7) {
        return date.toLocaleDateString([], { weekday: 'short' });
    } else {
        return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
    }
}

/**
 * Coerce any error-like value (Error, string, plain object, anything) into
 * a single human-readable string. Used by the chat window to display
 * errors that arrive over IPC, where the shape isn't predictable.
 */
export function formatError(error) {
    if (!error) return 'Unknown error';
    if (typeof error === 'string') return error;
    if (error.message) return error.message;
    if (error.toString && error.toString() !== '[object Object]') return error.toString();
    try {
        return JSON.stringify(error);
    } catch {
        return 'Unknown error';
    }
}

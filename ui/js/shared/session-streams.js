/**
 * Per-session stream registry.
 *
 * The chat window is a *viewport* over N independent agent sessions;
 * any number of them can be mid-turn at once (a send from the floating
 * window, a send from this window, a send from a peer window on a
 * shared session). This module owns per-session stream STATE so the
 * window can switch between them freely:
 *
 *   - the ACTIVE session renders live through the existing
 *     MessageStreamController path;
 *   - BACKGROUND sessions surface as spinner/unread badges in the
 *     sidebar;
 *   - switching into a mid-stream session re-attaches to the live
 *     stream (the text streamed so far comes from the backend's
 *     per-session accumulator via `get_session_stream_snapshot` — we
 *     deliberately do NOT mirror content into JS for background
 *     sessions; the backend/disk are the source of truth).
 *
 * What IS kept per session: the state machine (streaming/unread) and
 * tool-usage/source tracking. Tool chips have no peekable backend
 * equivalent, so each entry doubles as the `state` object handed to
 * `processToolCallUpdate` (shared/streaming-utils.js), which writes
 * `toolUsages`, `_toolCallIds`, `toolSources`, `_sourceDomains` onto it.
 *
 * Purely in-memory and per-window: completed turns live on disk
 * (loadSession replays them), in-flight text lives in the backend
 * accumulator.
 */

/** Stream lifecycle states. */
export const STREAM = {
    /** A user turn is in flight on this session. */
    STREAMING: 'streaming',
    /** Turn finished while the session was backgrounded; not yet viewed. */
    UNREAD: 'unread',
};

function newEntry(sessionId) {
    return {
        sessionId,
        state: STREAM.STREAMING,
        startedAt: Date.now(),
        // Written by processToolCallUpdate(event, entry):
        toolUsages: [],
        toolSources: [],
        _toolCallIds: new Set(),
        _sourceDomains: new Set(),
    };
}

export class SessionStreamRegistry {
    constructor() {
        /** @type {Map<string, object>} sessionId -> entry */
        this._streams = new Map();
        /** @type {Set<Function>} change subscribers */
        this._listeners = new Set();
    }

    /** Subscribe to registry changes (badge re-render). Returns unsubscribe. */
    onChange(fn) {
        this._listeners.add(fn);
        return () => this._listeners.delete(fn);
    }

    _notify(sessionId, kind) {
        for (const fn of this._listeners) {
            try {
                fn(sessionId, kind);
            } catch (e) {
                console.warn('[session-streams] onChange listener failed:', e);
            }
        }
    }

    /**
     * A user turn started on `sessionId`. Idempotent — a second begin on
     * an already-streaming session keeps the existing entry (tool chips
     * accumulated so far stay). A begin on an UNREAD entry starts a
     * fresh turn (the unread turn's text is already on disk).
     */
    begin(sessionId) {
        if (!sessionId) return null;
        let entry = this._streams.get(sessionId);
        if (entry && entry.state === STREAM.STREAMING) return entry;
        entry = newEntry(sessionId);
        this._streams.set(sessionId, entry);
        this._notify(sessionId, 'begin');
        return entry;
    }

    /**
     * A chunk arrived for `sessionId`. Ensures a STREAMING entry exists
     * (chunks can outrun the session_activity event for turns started
     * in another window) and returns it. Content is NOT stored — the
     * backend accumulator holds the text.
     */
    noteChunk(sessionId) {
        if (!sessionId) return null;
        const entry = this._streams.get(sessionId);
        if (entry && entry.state === STREAM.STREAMING) return entry;
        return this.begin(sessionId);
    }

    /**
     * Route a tool_call_update to the session's entry (creating one if
     * the tool call outran session_activity). Returns
     * `{ entry, updated, update }`; `updated`/`update` mirror
     * processToolCallUpdate's return.
     */
    trackTool(sessionId, event, processToolCallUpdate) {
        if (!sessionId) return { entry: null, updated: false, update: null };
        let entry = this._streams.get(sessionId);
        if (!entry || entry.state !== STREAM.STREAMING) {
            entry = this.begin(sessionId);
        }
        const { updated, update } = processToolCallUpdate(event, entry);
        return { entry, updated, update };
    }

    /**
     * The turn on `sessionId` completed. If the caller is currently
     * viewing this session (`viewing === true`) the entry is consumed
     * (returned + removed); otherwise it flips to UNREAD so the sidebar
     * can badge it (tool chips are kept for switch-in).
     */
    complete(sessionId, { viewing = false } = {}) {
        const entry = this._streams.get(sessionId);
        if (!entry) return null;
        if (viewing) {
            this._streams.delete(sessionId);
        } else {
            entry.state = STREAM.UNREAD;
        }
        this._notify(sessionId, 'complete');
        return entry;
    }

    /** The turn errored / was cancelled. Entry is dropped either way. */
    fail(sessionId) {
        const entry = this._streams.get(sessionId);
        if (!entry) return null;
        this._streams.delete(sessionId);
        this._notify(sessionId, 'fail');
        return entry;
    }

    /**
     * The user is now viewing `sessionId`. UNREAD entries are consumed
     * (returned + removed) — the on-disk session already has the final
     * text, the badge just needed clearing. STREAMING entries are
     * returned but kept: the caller re-attaches to the live stream.
     */
    markRead(sessionId) {
        const entry = this._streams.get(sessionId);
        if (!entry) return null;
        if (entry.state === STREAM.UNREAD) {
            this._streams.delete(sessionId);
            this._notify(sessionId, 'read');
        }
        return entry;
    }

    /** Current entry for a session, or null. */
    get(sessionId) {
        return this._streams.get(sessionId) || null;
    }

    /** True if a turn is currently in flight on `sessionId`. */
    isStreaming(sessionId) {
        return this._streams.get(sessionId)?.state === STREAM.STREAMING;
    }

    /** True if a background turn completed unviewed on `sessionId`. */
    isUnread(sessionId) {
        return this._streams.get(sessionId)?.state === STREAM.UNREAD;
    }

    /** Any session currently streaming? (Cheap global indicator.) */
    anyStreaming() {
        for (const e of this._streams.values()) {
            if (e.state === STREAM.STREAMING) return true;
        }
        return false;
    }

    /**
     * Snapshot of per-session badge states:
     * Map<sessionId, 'streaming'|'unread'>.
     */
    states() {
        const out = new Map();
        for (const [sid, e] of this._streams) {
            out.set(sid, e.state);
        }
        return out;
    }
}

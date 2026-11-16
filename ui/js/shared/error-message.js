/**
 * Extract a human-readable message from anything an `await invoke(...)` or
 * a regular catch block can hand us:
 *   - strings come back unchanged
 *   - Error instances → `.message` (or stringified, if message is empty)
 *   - AppError-shaped objects from Rust → `.message`
 *     (the Rust `AppError` serializes as `{ kind: "...", message: "..." }`)
 *   - other objects → JSON-encoded (fallback to String())
 *   - null / undefined → "Unknown error"
 *
 * Why this exists: Rust commands used to return `Result<_, String>`, and JS
 * could safely concat `'Error: ' + e` because `e` was a string. They now
 * return `Result<_, AppError>`, so the same concatenation produces
 * `"Error: [object Object]"`. Route every "render an error to the user"
 * site through this helper — it's a no-op for strings and a `.message`
 * extraction for objects.
 */
export function errMessage(e) {
    if (e == null) return 'Unknown error';
    if (typeof e === 'string') return e;
    if (e instanceof Error) return e.message || String(e);
    if (typeof e === 'object') {
        // AppError-shaped (`{ kind, message }`) and any other "has a message"
        // shape land here. Plain `.message` is the canonical field.
        if (typeof e.message === 'string' && e.message) return e.message;
        try {
            return JSON.stringify(e);
        } catch {
            return String(e);
        }
    }
    return String(e);
}

/**
 * The structured "kind" string from a Rust AppError, or null if the error
 * isn't AppError-shaped. Use for branching on specific error categories
 * without parsing the message text:
 *
 *     if (errKind(e) === 'connection_lost') { ... }
 *
 * Returns null for plain strings, Error instances, or objects without a
 * `kind` field — caller still gets a useful errMessage(e) in those cases.
 */
export function errKind(e) {
    if (e && typeof e === 'object' && typeof e.kind === 'string') {
        return e.kind;
    }
    return null;
}

/**
 * Build a labelled error message: `"<label>: <message>"` where the
 * message is extracted from `e` via `errMessage`. Saves the boilerplate
 * of `'X: ' + errMessage(e)` at every call site.
 *
 * Example:
 *     this.showError(errLabel('Reconnection failed', e));
 *     // → "Reconnection failed: Server closed the connection"
 */
export function errLabel(label, e) {
    return `${label}: ${errMessage(e)}`;
}

// Rate-limit wording in a `message_error` payload. Most emitters format
// their own text, so there is usually no `kind` to switch on.
const RATE_LIMIT_RE = /rate[\s_-]?limit|too many requests|\b429\b|quota exceeded/i;

/**
 * Whether a `message_error` payload (a string, or `{ kind, message }`) is a
 * rate limit. The floating mascot poses on it, and a rate-limited prompt is
 * never resent on Reconnect.
 */
export function isRateLimitPayload(payload) {
    const isObj = payload !== null && typeof payload === 'object';
    const kind = isObj ? payload.kind : null;
    const text = typeof payload === 'string' ? payload : isObj ? payload.message || '' : '';
    return kind === 'rate_limited' || RATE_LIMIT_RE.test(text);
}

/**
 * The session a `message_error` payload names: the session its failed turn
 * ran on. Only the object form carries one; a plain string gives null.
 */
export function errorSessionId(payload) {
    const isObj = payload !== null && typeof payload === 'object';
    const sid = isObj ? payload.sessionId : null;
    return typeof sid === 'string' && sid ? sid : null;
}

/**
 * The turn a `message_error` payload is about: the `turnId` the window sent
 * the prompt with (`newTurnId`), echoed by the backend. Only the object
 * form carries one; a plain string, or an object without it, gives null.
 */
export function errorTurnId(payload) {
    const isObj = payload !== null && typeof payload === 'object';
    const id = isObj ? payload.turnId : null;
    return typeof id === 'string' && id ? id : null;
}

/**
 * Whether `payload` is about another turn than `turnId`: it names a turn,
 * and not that one. Such an error must neither end the turn on screen nor
 * become the turn Reconnect resends. A payload naming no turn says nothing
 * either way (false).
 */
export function isErrorForOtherTurn(payload, turnId) {
    const named = errorTurnId(payload);
    return named !== null && named !== turnId;
}

/**
 * Whether the backend says the failed turn `turnId` may be sent again: the
 * error names that very turn, and says how it ended rules out that the
 * agent did anything (it answered with an error, or never got the prompt)
 * and that nothing got through (no text, tool call, permission request,
 * fs/terminal request or cancel). Only `resendable === true` on a payload
 * that names its session and this turn counts; a plain string never does,
 * nor does an object without a turn id, and a rate limit never is.
 */
export function isResendablePayload(payload, turnId) {
    return (
        typeof turnId === 'string' &&
        turnId !== '' &&
        errorTurnId(payload) === turnId &&
        errorSessionId(payload) !== null &&
        payload.resendable === true &&
        !isRateLimitPayload(payload)
    );
}

let _turnCounter = 0;

/**
 * A fresh id for one prompt a window sends (`turnId` of
 * send_message_streaming): its MESSAGE_ERROR echoes it, so an error is
 * matched to the turn it is about, never to a newer one on the same
 * session.
 */
export function newTurnId() {
    const c = globalThis.crypto;
    if (c && typeof c.randomUUID === 'function') return c.randomUUID();
    _turnCounter += 1;
    return `turn-${Date.now().toString(36)}-${_turnCounter}-${Math.random().toString(36).slice(2)}`;
}

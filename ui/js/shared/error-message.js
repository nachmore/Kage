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

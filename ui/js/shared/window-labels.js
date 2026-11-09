// Window labels — must agree with src/window_labels.rs.
// The Rust side has a unit test that asserts these labels match
// what's declared in tauri.conf.json. Add a label here AND there
// when introducing a new window.

export const WINDOW = {
    MAIN: 'main',
    FLOATING: 'floating',
    SETTINGS: 'settings',
    CONTEXT_MENU: 'context-menu',
    WELCOME: 'welcome',
    STORE: 'store',
    INLINE_ASSIST: 'inline-assist',
};

/// Per-conversation chat windows are labelled `chat-<uuid>`.
export const CHAT_PREFIX = 'chat-';

export function chatLabel(sessionUuid) {
    return `${CHAT_PREFIX}${sessionUuid}`;
}

export function isChatLabel(label) {
    return typeof label === 'string' && label.startsWith(CHAT_PREFIX);
}

/// Whether a label refers to a window that hosts a chat session
/// (`main` or any `chat-<uuid>` peer).
export function isSessionHostLabel(label) {
    return label === WINDOW.MAIN || isChatLabel(label);
}

/// Extension view windows are labelled `ext-view:<extensionId>:<viewId>`
/// (docs/design/EXTENSION_VIEWS.md §7.1). Build and match them only with
/// the helpers below; never inline the prefix.
export const EXT_VIEW_PREFIX = 'ext-view:';

// Same rules as `extensions::validate_extension_id` and
// `extensions::views::is_valid_view_id`. Neither allows `:`.
const EXTENSION_ID_RE = /^[a-z0-9][a-z0-9_-]{0,63}$/;
const VIEW_ID_RE = /^[a-z][a-z0-9-]{0,31}$/;

function validExtViewParts(extensionId, viewId) {
    return (
        typeof extensionId === 'string' &&
        typeof viewId === 'string' &&
        EXTENSION_ID_RE.test(extensionId) &&
        VIEW_ID_RE.test(viewId)
    );
}

/// The label for an extension's view window, or null when either id is
/// invalid (so a label can never carry a second `:` separator).
export function extViewLabel(extensionId, viewId) {
    return validExtViewParts(extensionId, viewId)
        ? `${EXT_VIEW_PREFIX}${extensionId}:${viewId}`
        : null;
}

/// `{ extensionId, viewId }` for a view window label, else null.
export function parseExtViewLabel(label) {
    if (typeof label !== 'string' || !label.startsWith(EXT_VIEW_PREFIX)) return null;
    const rest = label.slice(EXT_VIEW_PREFIX.length);
    const sep = rest.indexOf(':');
    if (sep < 0) return null;
    const extensionId = rest.slice(0, sep);
    const viewId = rest.slice(sep + 1);
    return validExtViewParts(extensionId, viewId) ? { extensionId, viewId } : null;
}

export function isExtViewLabel(label) {
    return parseExtViewLabel(label) !== null;
}

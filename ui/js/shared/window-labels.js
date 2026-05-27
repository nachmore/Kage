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

/**
 * Shared tool utility functions used across floating, chat, settings, and permissions UIs.
 */

/**
 * Get emoji icon for a tool kind (used in streaming tool chips)
 */
export function getToolIcon(kind) {
    const k = (kind || '').toLowerCase();
    if (k === 'search' || k === 'web_search') return '🔍';
    if (k === 'edit' || k === 'write') return '✏️';
    if (k === 'read') return '📖';
    if (k === 'shell' || k === 'terminal') return '💻';
    if (k === 'fetch' || k === 'web') return '🌐';
    if (k === 'extension') return '🧩';
    return '🔧';
}

/**
 * Get emoji for a tool name (used in permissions and settings)
 */
export function getToolEmoji(name) {
    const lower = (name || '').toLowerCase();
    // Extension tools — use the extension's icon if available
    if (lower.startsWith('ext:')) return '🧩';
    if (lower.includes('search')) return '🔍';
    if (lower.includes('fetch') || lower.includes('web')) return '🌐';
    if (lower.includes('read')) return '📖';
    if (lower.includes('write') || lower.includes('edit')) return '✏️';
    if (lower.includes('shell') || lower.includes('command') || lower.includes('terminal')) return '💻';
    if (lower.includes('aws') || lower.includes('cloud')) return '☁️';
    if (lower.includes('file')) return '📁';
    return '🔧';
}

/**
 * Escape HTML entities in a string
 */
export function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

/**
 * Strip internal Kage metadata tags from text for display purposes.
 * Removes <_kage_*> XML-style tags and [_KAGE_*] bracket markers.
 * These are injected by the app for agent context and should never be shown to users.
 */
export function stripKageTags(text) {
    if (!text) return text;
    return text
        .replace(/<_kage_[^>]*\/>\n?/g, '')   // <_kage_ctx app="..." title="..."/>
        .replace(/\[_KAGE_[A-Z_]*\][^\n]*\n?/g, '')  // [_KAGE_INLINE] Return ONLY...
        .trim();
}


/**
 * Map MCP/agent tool names to user-friendly descriptions for spinners.
 */
const TOOL_FRIENDLY_NAMES = {
    // Computer control — perception
    list_windows: 'Checking visible windows',
    list_all_windows: 'Enumerating all open windows',
    get_ui_tree: 'Reading application UI',
    find_elements: 'Searching for UI elements',
    get_focused_element: 'Checking focused element',
    get_element_text: 'Reading text content',
    get_element_children: 'Exploring UI details',
    // Computer control — actions
    click_element: 'Clicking element',
    set_value: 'Entering text',
    toggle_element: 'Toggling control',
    select_element: 'Selecting item',
    expand_element: 'Expanding menu',
    collapse_element: 'Collapsing menu',
    scroll_element: 'Scrolling',
    launch_app: 'Launching application',
    launch_and_get_tree: 'Launching application',
    click_and_get_tree: 'Clicking and reading UI',
    click_and_read_result: 'Clicking and reading result',
    type_and_get_tree: 'Typing text',
    // Computer control — fallback
    screenshot: 'Taking screenshot',
    click: 'Clicking',
    type_text: 'Typing',
    key_press: 'Pressing keys',
    drag: 'Dragging',
    scroll: 'Scrolling',
    move_mouse: 'Moving cursor',
    wait: 'Waiting',
    // Agent tools
    read: 'Reading file',
    write: 'Writing file',
    execute: 'Running command',
    search: 'Searching',
};

/**
 * Get a user-friendly description for a tool title (used in spinners).
 * Checks exact match, then substring match, then cleans up the raw title.
 */
export function getToolFriendlyName(title) {
    if (!title) return 'Working on it';
    if (TOOL_FRIENDLY_NAMES[title]) return TOOL_FRIENDLY_NAMES[title];
    for (const [key, friendly] of Object.entries(TOOL_FRIENDLY_NAMES)) {
        if (title.includes(key)) return friendly;
    }
    return title.replace(/^Running:\s*@\S+\s*/, '').replace(/_/g, ' ') || 'Working on it';
}

/**
 * Look up a friendly display name for an extension tool via the extension manager.
 */
export function getExtensionToolFriendlyName(extensionId, toolName, extensionManager) {
    if (extensionId && toolName && extensionManager) {
        const defs = extensionManager.getToolDefinitions();
        const extDef = defs.find(d => d.extensionId === extensionId);
        if (extDef?.tools) {
            const tool = extDef.tools.find(t => t.name === toolName);
            if (tool?.friendlyName) return tool.friendlyName;
        }
    }
    return 'Working on it';
}

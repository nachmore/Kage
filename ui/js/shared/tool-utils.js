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
 * Strip internal Kiro metadata tags from text for display purposes.
 * Removes <_kiro_*> XML-style tags and [_KIRO_*] bracket markers.
 * These are injected by the app for agent context and should never be shown to users.
 */
export function stripKiroTags(text) {
    if (!text) return text;
    return text
        .replace(/<_kiro_[^>]*\/>\n?/g, '')   // <_kiro_ctx app="..." title="..."/>
        .replace(/\[_KIRO_[A-Z_]*\][^\n]*\n?/g, '')  // [_KIRO_INLINE] Return ONLY...
        .trim();
}

/**
 * Clipboard History mode for the floating window.
 *
 * Activated by:
 * - Dedicated hotkey (emits 'clipboard_history_mode' event)
 * - Typing ">cb" or ">clipboard" in the input
 *
 * Shows a filterable list of clipboard history items.
 * Enter copies the selected item to clipboard.
 */

import { escapeHtml } from '../shared/tool-utils.js';

const CB_PREFIXES = ['>cb', '>clipboard'];

/** Check if the input text should trigger clipboard history mode */
export function isClipboardTrigger(query) {
    const lower = query.toLowerCase();
    return CB_PREFIXES.some(p => lower === p || lower.startsWith(p + ' '));
}

/** Extract the filter query from a clipboard trigger input */
export function getClipboardFilter(query) {
    const lower = query.toLowerCase();
    for (const p of CB_PREFIXES) {
        if (lower.startsWith(p + ' ')) return query.slice(p.length + 1).trim();
        if (lower === p) return '';
    }
    return '';
}

/** Fetch clipboard history from the backend */
export async function fetchClipboardHistory(invoke) {
    try {
        return await invoke('get_clipboard_history');
    } catch (e) {
        console.warn('[Clipboard] Failed to fetch history:', e);
        return [];
    }
}

/** Filter clipboard history entries by query */
export function filterClipboardHistory(entries, query) {
    if (!query) return entries;
    const lower = query.toLowerCase();
    return entries.filter(e => e.text.toLowerCase().includes(lower));
}

/** Format a timestamp for display */
function formatTime(isoString) {
    if (!isoString) return '';
    try {
        const d = new Date(isoString);
        const now = new Date();
        const diffMs = now - d;
        const diffMin = Math.floor(diffMs / 60000);
        if (diffMin < 1) return 'just now';
        if (diffMin < 60) return `${diffMin}m ago`;
        const diffHr = Math.floor(diffMin / 60);
        if (diffHr < 24) return `${diffHr}h ago`;
        return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
    } catch { return ''; }
}

/** Truncate text for preview */
function truncate(text, maxLen = 80) {
    const oneLine = text.replace(/\n/g, ' ↵ ').trim();
    if (oneLine.length <= maxLen) return oneLine;
    return oneLine.slice(0, maxLen) + '…';
}

/**
 * Render clipboard history entries into the suggestion container.
 * Returns the selected index (0 if items exist, -1 if empty).
 */
export function renderClipboardHistory(entries, container, currentMatches, resizeWindow) {
    currentMatches.length = 0;
    container.innerHTML = '';

    if (entries.length === 0) {
        container.innerHTML = `
            <div class="app-suggestion-item" style="opacity:0.6;pointer-events:none;">
                <div class="app-icon">📋</div>
                <div class="app-info">
                    <div class="app-name">No clipboard history</div>
                    <div class="app-description">Enable clipboard history in Windows Settings → System → Clipboard</div>
                </div>
            </div>
        `;
        container.classList.add('visible');
        resizeWindow();
        return -1;
    }

    for (let i = 0; i < entries.length; i++) {
        const entry = entries[i];
        const isImage = entry.content_type === 'image';
        const icon = isImage ? '🖼️' : '📄';
        const preview = isImage ? '[Image]' : truncate(entry.text);
        const time = formatTime(entry.timestamp);

        const item = document.createElement('div');
        item.className = 'app-suggestion-item' + (i === 0 ? ' selected' : '');
        item.innerHTML = `
            <div class="app-icon">${icon}</div>
            <div class="app-info">
                <div class="app-name">${escapeHtml(preview)}</div>
                <div class="app-description">${time}</div>
            </div>
        `;
        container.appendChild(item);

        currentMatches.push({
            type: 'clipboard',
            label: preview,
            name: entry.text,
            data: entry,
        });
    }

    container.classList.add('visible');
    container.scrollTop = 0;
    resizeWindow();
    return 0;
}

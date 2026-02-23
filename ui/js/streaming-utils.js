/**
 * Shared streaming utilities for tool/source tracking.
 * Used by both FloatingApp and ChatApp to avoid duplicating
 * the ACP event parsing and source extraction logic.
 */

/**
 * Parse a tool_call_update event and track tool usage + sources.
 * @param {Object} event - Tauri event payload
 * @param {Object} state - { toolUsages: [], toolSources: [] }
 * @returns {{ updated: boolean, update: Object|null }} whether state changed
 */
export function processToolCallUpdate(event, state) {
    const notification = event.payload;
    const update = notification?.params?.update;
    if (!update) return { updated: false, update: null };

    let updated = false;

    // Track tool usage
    if (update.title && update.toolCallId) {
        if (!state.toolUsages.find(t => t.toolCallId === update.toolCallId)) {
            state.toolUsages.push({
                toolCallId: update.toolCallId,
                title: update.title,
                kind: update.kind
            });
            updated = true;
        }
    }

    // Extract sources from search results
    const rawOutput = update.rawOutput;
    if (rawOutput && (update.kind === 'search' || update.title?.toLowerCase().includes('search'))) {
        extractSources(rawOutput, state);
        updated = true;
    }

    // Extract sources from content URLs
    if (update.content && Array.isArray(update.content)) {
        for (const item of update.content) {
            if (item.type === 'content' && item.content?.text) {
                extractSourcesFromText(item.content.text, state);
            }
        }
    }

    return { updated, update };
}

/**
 * Extract source URLs from raw search output.
 */
export function extractSources(rawOutput, state) {
    const tryExtract = (results) => {
        if (Array.isArray(results)) {
            for (const r of results) {
                if (r.url) addSource(r.url, r.title, r.domain, state);
            }
        }
    };

    if (rawOutput?.items && Array.isArray(rawOutput.items)) {
        for (const item of rawOutput.items) {
            tryExtract(item?.Json?.results || item?.results);
        }
    } else if (Array.isArray(rawOutput)) {
        tryExtract(rawOutput);
    } else if (typeof rawOutput === 'object') {
        tryExtract(rawOutput.results || rawOutput.searchResults);
    }
}

/**
 * Extract source URLs from markdown-style links in text.
 */
export function extractSourcesFromText(text, state) {
    const linkRegex = /\[([^\]]*)\]\((https?:\/\/[^\s)]+)\)/g;
    let match;
    while ((match = linkRegex.exec(text)) !== null) {
        addSource(match[2], match[1], null, state);
    }
}

/**
 * Add a source URL, deduplicating by domain.
 */
export function addSource(url, title, domainHint, state) {
    try {
        const parsed = new URL(url);
        const domain = domainHint || parsed.hostname.replace(/^www\./, '');
        if (!state.toolSources.find(s => s.domain === domain)) {
            const initials = domain.split('.')[0].substring(0, 2).toUpperCase();
            let hash = 0;
            for (let i = 0; i < domain.length; i++) {
                hash = domain.charCodeAt(i) + ((hash << 5) - hash);
            }
            const hue = Math.abs(hash) % 360;
            state.toolSources.push({
                url, domain,
                title: title || domain,
                initials,
                color: `hsl(${hue}, 55%, 45%)`,
                favicon: `https://www.google.com/s2/favicons?domain=${domain}&sz=32`
            });
        }
    } catch { /* invalid URL */ }
}

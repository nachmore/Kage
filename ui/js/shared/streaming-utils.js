/**
 * Shared streaming utilities for tool/source tracking.
 * Used by both FloatingApp and ChatApp to avoid duplicating
 * the ACP event parsing and source extraction logic.
 */

import { tHtml } from './i18n.js';
import { escapeAttr, escapeHtml, getToolIcon } from './tool-utils.js';

// `escapeAttr` is re-exported here so existing call sites that import
// it from this module keep working. Canonical home is `tool-utils.js`.
export { escapeAttr };

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
        if (!state._toolCallIds) state._toolCallIds = new Set();
        if (!state._toolCallIds.has(update.toolCallId)) {
            state._toolCallIds.add(update.toolCallId);
            state.toolUsages.push({
                toolCallId: update.toolCallId,
                title: update.title,
                kind: update.kind,
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
        if (!state._sourceDomains) state._sourceDomains = new Set();
        if (!state._sourceDomains.has(domain)) {
            state._sourceDomains.add(domain);
            const initials = domain.split('.')[0].substring(0, 2).toUpperCase();
            let hash = 0;
            for (let i = 0; i < domain.length; i++) {
                hash = domain.charCodeAt(i) + ((hash << 5) - hash);
            }
            const hue = Math.abs(hash) % 360;
            state.toolSources.push({
                url,
                domain,
                title: title || domain,
                initials,
                color: `hsl(${hue}, 55%, 45%)`,
                favicon: `https://www.google.com/s2/favicons?domain=${domain}&sz=32`,
            });
        }
    } catch {
        /* invalid URL */
    }
}

/**
 * Generate HTML for tool usage chips.
 * @param {Array} toolUsages - [{ toolCallId, title, kind }]
 * @returns {string} HTML string
 */
export function renderToolChipsHtml(toolUsages) {
    // Deduplicate by title — show each tool once with a count badge
    const grouped = new Map();
    for (const t of toolUsages) {
        const key = t.title;
        if (grouped.has(key)) {
            grouped.get(key).count++;
        } else {
            grouped.set(key, { ...t, count: 1 });
        }
    }
    return Array.from(grouped.values())
        .map((t) => {
            const isExt = t.title.startsWith('ext:');
            let displayName, tooltip;
            if (isExt) {
                const parts = t.title.substring(4); // remove "ext:"
                const extId = parts.split('/')[0];
                displayName = extId.charAt(0).toUpperCase() + extId.slice(1);
                tooltip = `Extension: ${parts}`;
            } else {
                displayName = t.title;
                tooltip = `Tool: ${t.title}`;
            }
            const badge =
                t.count > 1 ? `<span class="tool-chip-count">\u00d7${t.count}</span>` : '';
            return `
        <span class="source-chip tool-chip" title="${escapeAttr(tooltip + (t.count > 1 ? ' (' + t.count + ' calls)' : ''))}">
            <span class="tool-chip-icon">${getToolIcon(t.kind)}</span>
            <span class="source-domain">${escapeHtml(displayName)}</span>${badge}
        </span>
    `;
        })
        .join('');
}

/**
 * Generate HTML for source domain chips (clickable links).
 *
 * Source URLs/titles/favicons originate from agent-streamed search results and
 * markdown links — i.e. attacker-influenceable content. We never interpolate
 * any of those values into JS contexts (no inline onclick); the URL rides on
 * a `data-url` attribute (HTML-escaped) and a delegated click handler installed
 * via `attachSourceClickHandler` reads it back and routes through `open_url`.
 *
 * @param {Array} toolSources - [{ url, domain, title, initials, color, favicon }]
 * @returns {string} HTML string
 */
export function renderSourceChipsHtml(toolSources) {
    return toolSources
        .map(
            (s) => `
        <a class="source-chip" href="#" data-url="${escapeAttr(s.url)}" title="${escapeAttr(s.title)}">
            <span class="source-icon-wrapper">
                <span class="source-initials" style="background:${escapeAttr(s.color)}">${escapeHtml(s.initials)}</span>
                <img class="source-favicon" src="${escapeAttr(s.favicon)}" alt="" onload="this.previousElementSibling.style.display='none'" onerror="this.style.display='none'">
            </span>
            <span class="source-domain">${escapeHtml(s.domain)}</span>
        </a>
    `
        )
        .join('');
}

/**
 * Generate HTML for compact source bubbles (floating window streaming state).
 * @param {Array} toolUsages
 * @param {Array} toolSources
 * @returns {string} HTML string
 */
export function renderSourceBubblesHtml(toolUsages, toolSources) {
    // Deduplicate tools by title
    const grouped = new Map();
    for (const t of toolUsages) {
        if (!grouped.has(t.title)) grouped.set(t.title, t);
    }
    const uniqueTools = Array.from(grouped.values());

    const toolBubbles = uniqueTools
        .map(
            (t, i) => `
        <span class="source-bubble tool-bubble" title="${escapeAttr(t.title)}" style="animation-delay: ${i * 0.08}s">
            <span class="tool-chip-icon" style="font-size: 18px;">${getToolIcon(t.kind)}</span>
        </span>
    `
        )
        .join('');

    const offset = uniqueTools.length;
    const sourceBubbles = toolSources
        .map(
            (s, i) => `
        <a class="source-bubble" href="#" data-url="${escapeAttr(s.url)}" title="${escapeAttr(s.title)}" style="animation-delay: ${(offset + i) * 0.08}s">
            <span class="source-icon-wrapper">
                <span class="source-initials" style="background:${escapeAttr(s.color)}">${escapeHtml(s.initials)}</span>
                <img class="source-favicon" src="${escapeAttr(s.favicon)}" alt="" onload="this.previousElementSibling.style.display='none'" onerror="this.style.display='none'">
            </span>
        </a>
    `
        )
        .join('');

    return toolBubbles + sourceBubbles;
}

/**
 * Install a delegated click handler that intercepts source-chip / source-bubble
 * clicks and routes the `data-url` attribute through `open_url`. Idempotent:
 * a container already wired is left alone. Pair with `renderSourceChipsHtml` /
 * `renderSourceBubblesHtml` whose links carry `data-url` instead of an inline
 * onclick (the inline form was a small XSS sink — agent-controlled URLs were
 * interpolated into a JS string with brittle quote escaping).
 *
 * @param {Element} container - element holding `.source-chip` / `.source-bubble`
 * @param {Function} invoke - the Tauri `invoke` from `tauri-init.js`
 */
export function attachSourceClickHandler(container, invoke) {
    if (!container || container.__kageSourceClickWired) return;
    container.__kageSourceClickWired = true;
    container.addEventListener('click', (event) => {
        const link = event.target.closest('[data-url]');
        if (!link || !container.contains(link)) return;
        const url = link.getAttribute('data-url');
        if (!url) return;
        event.preventDefault();
        invoke('open_url', { url }).catch(() => {});
    });
}

/**
 * Get the appropriate error/info message for a session reset event.
 * @param {Object} data - event payload data
 * @returns {string} message
 */
export function getSessionResetMessage(data) {
    if (data?.reason === 'image_unsupported') {
        return data.reconnected
            ? "🖼️ The current model doesn't support images. A new session has been started — try switching to a vision-capable model."
            : "🖼️ The current model doesn't support images and the connection could not be restored. Please reconnect manually.";
    }
    return 'Session was reset due to an error.';
}

/**
 * Detect an automation plan in the LLM response text.
 * Looks for a ```automation_plan JSON code block.
 * @param {string} text - The response text to scan
 * @returns {Array|null} Parsed plan array, or null if not found
 */
export function detectAutomationPlan(text) {
    if (!text) return null;
    // Match complete ```automation_plan ... ``` blocks
    const regex = /```automation_plan\s*\n([\s\S]*?)```/;
    const match = text.match(regex);
    if (!match) return null;
    try {
        const parsed = JSON.parse(match[1].trim());
        if (Array.isArray(parsed) && parsed.length > 0 && parsed[0].task) {
            return parsed;
        }
    } catch {
        /* invalid JSON */
    }
    return null;
}

/**
 * Incrementally parse automation plan steps from a streaming response.
 * Extracts individual step objects as they appear, even before the JSON array is complete.
 * @param {string} text - The streaming response text
 * @returns {Array|null} Array of parsed steps so far, or null if no plan block detected
 */
export function detectAutomationPlanIncremental(text) {
    if (!text?.includes('```automation_plan')) return null;

    // Extract everything after the code fence opener
    const fenceStart = text.indexOf('```automation_plan');
    if (fenceStart === -1) return null;
    const afterFence = text.substring(fenceStart + '```automation_plan'.length);

    // Try to find individual step objects using regex
    const stepRegex =
        /\{\s*"step"\s*:\s*(\d+)\s*,\s*"task"\s*:\s*"([^"]*)"(?:\s*,\s*"details"\s*:\s*"([^"]*)")?\s*\}/g;
    const steps = [];
    let match;
    while ((match = stepRegex.exec(afterFence)) !== null) {
        steps.push({
            step: parseInt(match[1], 10),
            task: match[2],
            details: match[3] || '',
        });
    }

    return steps.length > 0 ? steps : null;
}

/**
 * Convert an automation plan + statuses into the taskplan format
 * used by createTaskPlanElement.
 * @param {Array} plan - Array of { step, task, details }
 * @param {Object} stepStatuses - Map of step number to status ('pending'|'running'|'done'|'failed')
 * @param {Object} stepResults - Map of step number to result text
 * @returns {Array<{status: string, description: string, detail: string}>}
 */
export function automationPlanToTasks(plan, stepStatuses = {}, stepResults = {}) {
    return plan.map((s) => {
        const rawStatus = stepStatuses[s.step] || 'pending';
        // Map our statuses to taskplan statuses
        const statusMap = {
            pending: 'pending',
            running: 'active',
            done: 'done',
            failed: 'error',
            stopped: 'stopped',
        };
        const status = statusMap[rawStatus] || 'pending';
        const result = stepResults[s.step] || '';
        const cancelled = rawStatus === 'stopped';
        // Combine details and result for the detail field
        let detail = s.details || '';
        if (result && !cancelled) {
            detail = result;
        }
        return { status, description: s.task, detail, cancelled };
    });
}

/**
 * Detect a complete ```extension_tool_call``` fence in streaming text.
 * Returns the parsed call object or null if not found/incomplete.
 * @param {string} text - Accumulated streaming text
 * @returns {{ extension: string, tool: string, params: object }|null}
 */
export function detectExtensionToolCall(text) {
    if (!text?.includes('```extension_tool_call')) return null;

    const fenceStart = text.indexOf('```extension_tool_call');
    const afterOpener = text.substring(fenceStart + '```extension_tool_call'.length);

    // Need the closing fence to consider it complete
    const closingIdx = afterOpener.indexOf('```');
    if (closingIdx === -1) return null;

    const jsonStr = afterOpener.substring(0, closingIdx).trim();
    try {
        const parsed = JSON.parse(jsonStr);
        if (parsed.extension && parsed.tool) {
            return {
                extension: parsed.extension,
                tool: parsed.tool,
                params: parsed.params || {},
            };
        }
    } catch (e) {
        // JSON parse failed — log for debugging
        console.warn(
            '[ExtToolCall] JSON parse failed:',
            e.message,
            'length:',
            jsonStr.length,
            'start:',
            jsonStr.substring(0, 100)
        );
    }
    return null;
}

/**
 * Detect an in-progress (incomplete) extension tool call fence.
 * Used to show the loading indicator while the fence is being streamed.
 * @param {string} text
 * @returns {{ extension?: string, tool?: string, inProgress: boolean }}
 */
export function detectExtensionToolCallIncremental(text) {
    if (!text?.includes('```extension_tool_call')) return null;

    const fenceStart = text.indexOf('```extension_tool_call');
    const afterOpener = text.substring(fenceStart + '```extension_tool_call'.length);

    // If we have a closing fence, it's complete — not "incremental" anymore
    const closingIdx = afterOpener.indexOf('```');
    if (closingIdx !== -1) return null;

    // Try to extract partial JSON for the indicator
    const jsonStr = afterOpener.trim();
    let extension, tool;
    try {
        // Try parsing even if incomplete — might have enough
        const partial = JSON.parse(jsonStr);
        extension = partial.extension;
        tool = partial.tool;
    } catch {
        // Try regex extraction for partial JSON
        const extMatch = jsonStr.match(/"extension"\s*:\s*"([^"]*)"/);
        const toolMatch = jsonStr.match(/"tool"\s*:\s*"([^"]*)"/);
        if (extMatch) extension = extMatch[1];
        if (toolMatch) tool = toolMatch[1];
    }

    return { extension, tool, inProgress: true };
}

/**
 * Render an extension tool call chip (loading indicator or completed).
 * @param {object} info - { extension, tool, icon, status: 'loading'|'done'|'error' }
 * @returns {string} HTML string
 */
export function renderExtensionToolChipHtml(info) {
    const icon = info.icon || '🧩';
    const toolLabel = info.tool ? `${info.extension}/${info.tool}` : info.extension || 'extension';
    const statusIcon = info.status === 'loading' ? '⏳' : info.status === 'error' ? '❌' : '✅';
    return `
        <span class="source-chip tool-chip ext-tool-chip ext-tool-${info.status || 'loading'}" title="${tHtml('shared.streaming.ext_tool.title', { label: toolLabel })}">
            <span class="tool-chip-icon">${icon}</span>
            <span class="source-domain">${statusIcon} ${escapeHtml(toolLabel)}</span>
        </span>
    `;
}

/**
 * Detect and extract a ```suggested_actions``` block from response text.
 * Returns { actions: Array<{label, prompt}>, cleanText: string } or null.
 */
export function extractSuggestedActions(text) {
    if (!text?.includes('```suggested_actions')) return null;

    const regex = /```suggested_actions\s*\n([\s\S]*?)```/;
    const match = text.match(regex);
    if (!match) return null;

    try {
        const parsed = JSON.parse(match[1].trim());
        if (Array.isArray(parsed) && parsed.length > 0 && parsed[0].label) {
            // Strip the block from the visible text
            const cleanText = text.replace(regex, '').trim();
            return { actions: parsed, cleanText };
        }
    } catch {
        /* invalid JSON */
    }
    return null;
}

/**
 * Unified search engine for the floating window.
 * Queries all sources in parallel, merges results with frecency scoring.
 * Extension-provided search results come from the ExtensionManager.
 */

import { matchCommands, matchSlashCommands, matchCommandsByName } from './commands.js';

// --- Frecency store ---

let _frecencyData = {}; // { resultId: { count, lastUsed, prefixes: { prefix: count } } }
let _frecencyLoaded = false;

/**
 * Record that the user selected a result for a given query.
 */
export function recordSelection(query, resultId, invoke) {
    const prefix = query.toLowerCase().substring(0, 6); // use first 6 chars as prefix key
    if (!_frecencyData[resultId]) {
        _frecencyData[resultId] = { count: 0, lastUsed: 0, prefixes: {} };
    }
    const entry = _frecencyData[resultId];
    entry.count++;
    entry.lastUsed = Date.now();
    entry.prefixes[prefix] = (entry.prefixes[prefix] || 0) + 1;

    // Persist (debounced)
    _debounceSave(invoke);
}

/**
 * Get a frecency boost score for a result given the current query.
 */
function getFrecencyBoost(query, resultId) {
    const entry = _frecencyData[resultId];
    if (!entry) return 0;

    const prefix = query.toLowerCase().substring(0, 6);
    const prefixCount = entry.prefixes[prefix] || 0;
    const totalCount = entry.count;

    // Recency decay: full weight within 7 days, half at 30 days, quarter at 90 days
    const ageMs = Date.now() - entry.lastUsed;
    const ageDays = ageMs / 86400000;
    let recencyMultiplier = 1;
    if (ageDays > 90) recencyMultiplier = 0.1;
    else if (ageDays > 30) recencyMultiplier = 0.25;
    else if (ageDays > 7) recencyMultiplier = 0.5;

    // Prefix-specific count matters more than total count
    return (prefixCount * 15 + totalCount * 3) * recencyMultiplier;
}

let _saveTimer = null;
function _debounceSave(invoke) {
    if (_saveTimer) clearTimeout(_saveTimer);
    _saveTimer = setTimeout(() => {
        // Prune entries older than 90 days
        const cutoff = Date.now() - 90 * 86400000;
        for (const [id, entry] of Object.entries(_frecencyData)) {
            if (entry.lastUsed < cutoff) delete _frecencyData[id];
        }
        // Save via Rust (write to config dir)
        invoke('save_frecency', { data: JSON.stringify(_frecencyData) }).catch(() => {});
    }, 2000);
}

export async function loadFrecency(invoke) {
    if (_frecencyLoaded) return;
    try {
        const json = await invoke('load_frecency');
        if (json) _frecencyData = JSON.parse(json);
    } catch {}
    _frecencyLoaded = true;
}

// --- Extension manager reference ---
let _extensionManager = null;

/**
 * Set the extension manager instance (called once from floating-main.js after init).
 */
export function setExtensionManager(mgr) {
    _extensionManager = mgr;
}

// --- Unified search ---

/**
 * Run all search sources and return a merged, scored, deduplicated result list.
 * @param {string} query - trimmed user input
 * @param {function} invoke - Tauri invoke function
 * @param {object} shortcuts - loaded shortcuts array
 * @returns {Promise<Array>} sorted results, highest score first
 */
export async function unifiedSearch(query, invoke, shortcuts) {
    if (!query) return [];

    const results = [];

    // --- Extension search providers (replaces hardcoded color/math/devtools/timer) ---
    if (_extensionManager) {
        const extResults = _extensionManager.matchAll(query);
        results.push(...extResults);

        // Async extension results (e.g. hashing)
        const asyncResults = await _extensionManager.matchAllAsync(query);
        results.push(...asyncResults);
    }

    // > commands
    if (query.startsWith('>')) {
        const commands = matchCommands(query);
        if (commands) {
            for (const cmd of commands) {
                results.push({
                    id: 'cmd:' + (cmd.name || cmd.label),
                    type: cmd.type || 'command',
                    label: cmd.name || cmd.label,
                    description: cmd.description || '',
                    icon: cmd.icon || '⚡',
                    score: 90,
                    data: cmd,
                });
            }
        }
        // Don't search other sources for > commands
        return _applyFrecency(results, query);
    }

    // / slash commands
    if (query.startsWith('/')) {
        const slashCmds = matchSlashCommands(query);
        if (slashCmds) {
            for (const cmd of slashCmds) {
                results.push({
                    id: 'slash:' + (cmd.name || cmd.label),
                    type: cmd.type || 'slash',
                    label: cmd.name || cmd.label,
                    description: cmd.description || '',
                    icon: cmd.icon || '/',
                    score: 90,
                    data: cmd,
                });
            }
        }
        return _applyFrecency(results, query);
    }

    // Command name matches (without > prefix)
    const cmdMatches = matchCommandsByName(query);
    for (const cmd of cmdMatches) {
        results.push({
            id: 'cmd:' + (cmd.name || cmd.label),
            type: cmd.type || 'command',
            label: cmd.name || cmd.label,
            description: cmd.description || '',
            icon: cmd.icon || '⚡',
            score: 70,
            data: cmd,
        });
    }

    // Shortcuts
    if (shortcuts && shortcuts.length > 0) {
        const lower = query.toLowerCase();
        for (const sc of shortcuts) {
            const scLower = sc.shortcut?.toLowerCase() || '';
            const nameLower = sc.name?.toLowerCase() || '';
            if (scLower.startsWith(lower) || nameLower.startsWith(lower) || nameLower.includes(lower)) {
                // Extract args: text after the shortcut trigger word
                const triggerLen = scLower.startsWith(lower) ? sc.shortcut.length : sc.name.length;
                const rawArgs = query.length > triggerLen ? query.substring(triggerLen).trim() : '';
                const argsArray = rawArgs ? rawArgs.split(/\s+/) : [];

                // Build description with param hints
                let desc = '⚡ ' + sc.shortcut;
                const templates = [sc.url, sc.prompt, sc.arguments, sc.script].filter(Boolean).join(' ');
                const reqParams = new Set();
                let m;
                const re = /\{(\d+)\}(?!\?)/g;
                while ((m = re.exec(templates)) !== null) reqParams.add(parseInt(m[1]));
                if (reqParams.size > 0 && argsArray.length < Math.max(...reqParams) + 1) {
                    const needed = Math.max(...reqParams) + 1;
                    desc += ` · ${needed - argsArray.length} param${needed - argsArray.length > 1 ? 's' : ''} needed`;
                }

                results.push({
                    id: 'shortcut:' + sc.name,
                    type: 'shortcut',
                    label: sc.name,
                    description: desc,
                    icon: '⚡',
                    score: scLower === lower || nameLower === lower ? 85 : 65,
                    data: { shortcut: sc, args: argsArray },
                });
            }
        }
    }

    // --- Rust-side search (one IPC call returning all matches as JSON array) ---
    try {
        const rustJson = await invoke('handle_floating_input', { input: query });
        const rustResults = JSON.parse(rustJson);
        for (const r of rustResults) {
            if (r.type === 'url') {
                results.push({ id: 'url:' + r.value, type: 'url', label: 'Open in browser', description: r.value, icon: '🌐', score: r.score || 88, data: { value: r.value } });
            } else if (r.type === 'path') {
                results.push({ id: 'path:' + r.value, type: 'path', label: r.pathType === 'file' ? 'Open File' : 'Open Folder', description: r.value, icon: r.pathType === 'file' ? '📄' : '📁', score: r.score || 87, data: { value: r.value, pathType: r.pathType } });
            } else if (r.type === 'system') {
                results.push({ id: 'system:' + r.cmdId, type: 'system', label: r.cmdLabel, description: r.needsConfirm ? 'Press Enter to select' : 'Press Enter to execute', icon: '⚙️', score: r.score || 86, data: { cmdId: r.cmdId, cmdLabel: r.cmdLabel, needsConfirm: r.needsConfirm } });
            } else if (r.type === 'app') {
                results.push({ id: 'app:' + r.name, type: 'app', label: r.name, description: '', icon: '', score: r.score || 75, data: { name: r.name, icon_base64: r.icon_base64, emoji_icon: r.emoji_icon } });
            }
        }
    } catch {}

    return _applyFrecency(results, query);
}

function _applyFrecency(results, query) {
    for (const r of results) {
        r.score += getFrecencyBoost(query, r.id);
    }
    results.sort((a, b) => b.score - a.score);
    return results.slice(0, 7);
}

// --- Unified suggestion renderer ---

/**
 * Render unified search results into the suggestion container.
 * Returns the selectedIndex (0 if results exist, -1 if empty).
 */
export function renderUnifiedResults(results, container, currentMatches, resizeWindow) {
    container.innerHTML = '';
    container.scrollTop = 0;
    currentMatches.length = 0;

    if (!results.length) {
        container.classList.remove('visible');
        return -1;
    }

    for (let i = 0; i < results.length; i++) {
        const r = results[i];
        currentMatches.push(r);

        const item = document.createElement('div');
        item.className = 'app-suggestion-item' + (i === 0 ? ' selected' : '');

        let iconHtml;
        if (r.type === 'app' && r.data?.icon_base64) {
            const src = r.data.icon_base64.startsWith('data:') ? r.data.icon_base64 : 'data:image/png;base64,' + r.data.icon_base64;
            iconHtml = `<img src="${src}" class="app-icon-img" onerror="this.style.display='none';this.nextElementSibling.style.display='flex'"><div class="app-icon" style="display:none">${r.data.emoji_icon || r.label.charAt(0).toUpperCase()}</div>`;
        } else if (r.type === 'color') {
            const hex = '#' + [r.data.r,r.data.g,r.data.b].map(c => c.toString(16).padStart(2,'0')).join('');
            iconHtml = `<div class="app-icon" style="position:relative;background:${hex};border:2px solid rgba(255,255,255,0.2);cursor:pointer;">` +
                `<input type="color" value="${hex}" style="position:absolute;top:0;left:0;width:100%;height:100%;opacity:0;cursor:pointer;" ` +
                `data-color-picker="true"></div>`;
        } else {
            iconHtml = `<div class="app-icon">${r.icon || r.label.charAt(0)}</div>`;
        }

        item.innerHTML = `
            ${iconHtml}
            <div class="app-info">
                <div class="app-name">${_escapeHtml(r.label)}</div>
                ${r.description ? `<div class="app-description">${_escapeHtml(r.description)}</div>` : ''}
            </div>
        `;

        container.appendChild(item);
    }

    container.classList.add('visible');

    // Wire up color picker inputs
    container.querySelectorAll('input[data-color-picker]').forEach(picker => {
        const item = picker.closest('.app-suggestion-item');
        const swatch = picker.parentElement;
        const nameEl = item?.querySelector('.app-name');
        const descEl = item?.querySelector('.app-description');
        const idx = Array.from(container.children).indexOf(item);

        picker.addEventListener('input', (e) => {
            const newHex = e.target.value;
            swatch.style.background = newHex;
            const nr = parseInt(newHex.slice(1,3),16), ng = parseInt(newHex.slice(3,5),16), nb = parseInt(newHex.slice(5,7),16);
            if (nameEl) nameEl.textContent = newHex.toUpperCase();
            // Update stored match data so Enter copies the picked color
            if (idx >= 0 && currentMatches[idx]?.type === 'color') {
                currentMatches[idx].data = { r: nr, g: ng, b: nb, source: 'picker' };
                currentMatches[idx].label = newHex.toUpperCase();
            }
        });
    });

    setTimeout(() => resizeWindow(), 10);
    return 0;
}

function _escapeHtml(str) {
    return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

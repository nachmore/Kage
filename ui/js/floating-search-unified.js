/**
 * Unified search engine for the floating window.
 * Queries all sources in parallel, merges results with frecency scoring.
 */

import { parseColor } from './floating-color.js';
import { evaluateMath } from './math-eval.js';
import { matchDevTool, computeHash } from './floating-devtools.js';
import { parseTimerCommand } from './floating-timer.js';
import { matchCommands, matchSlashCommands, matchCommandsByName } from './floating-commands.js';

// --- Frecency store ---

const FRECENCY_FILE = 'search-frecency.json';
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

// --- Unified result type ---
// { id: string, type: string, label: string, description: string, icon: string, score: number, data: any }

// --- Config cache ---
let _configCache = null;
let _configCacheTime = 0;
const CONFIG_CACHE_TTL = 5000; // 5 seconds

async function getConfig(invoke) {
    const now = Date.now();
    if (_configCache && now - _configCacheTime < CONFIG_CACHE_TTL) return _configCache;
    try {
        _configCache = await invoke('get_config');
        _configCacheTime = now;
    } catch {}
    return _configCache || {};
}

export function invalidateConfigCache() {
    _configCache = null;
    _configCacheTime = 0;
}

// --- Unified search ---

/**
 * Run all search sources and return a merged, scored, deduplicated result list.
 * @param {string} query - trimmed user input
 * @param {function} invoke - Tauri invoke function
 * @param {object} shortcuts - loaded shortcuts array
 * @param {object} mathConfig - math config
 * @returns {Promise<Array>} sorted results, highest score first
 */
export async function unifiedSearch(query, invoke, shortcuts, mathConfig) {
    if (!query) return [];

    const config = await getConfig(invoke);
    const results = [];

    // --- JS-side instant matchers (run synchronously) ---

    // Color
    if (config.color_picker?.enabled !== false) {
        const color = parseColor(query);
        if (color) {
            const { r, g, b } = color;
            const hex = '#' + [r,g,b].map(c => c.toString(16).padStart(2,'0')).join('');
            results.push({
                id: 'color:' + hex,
                type: 'color',
                label: hex.toUpperCase(),
                description: 'Color preview · Enter to copy',
                icon: '🎨',
                score: 95,
                data: color,
            });
        }
    }

    // Math / unit conversion
    if (mathConfig?.enabled !== false) {
        const mathResult = evaluateMath(query, mathConfig?.precision || 0);
        if (mathResult) {
            let display = mathResult.display;
            if (mathConfig?.thousands_separator) {
                const parts = display.split('.');
                parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, ',');
                display = parts.join('.');
            }
            results.push({
                id: 'math',
                type: 'math',
                label: '= ' + display,
                description: 'Press Enter to copy result',
                icon: '🧮',
                score: 93,
                data: { value: display, raw: mathResult.result },
            });
        }
    }

    // Dev tools
    if (config.dev_tools?.enabled !== false) {
        const dt = matchDevTool(query, config.dev_tools || {});
        if (dt) {
            if (dt.type === 'devtool_async') {
                // Hash — compute async but still include
                try {
                    const hash = await computeHash(dt.algo, dt.text);
                    if (hash) {
                        results.push({
                            id: 'devtool:' + dt.algo,
                            type: 'devtool',
                            label: hash,
                            description: dt.label + ' · Enter to copy',
                            icon: dt.icon,
                            score: 92,
                            data: { value: hash },
                        });
                    }
                } catch {}
            } else {
                results.push({
                    id: 'devtool:' + dt.label,
                    type: 'devtool',
                    label: dt.value,
                    description: dt.label + ' · ' + dt.description,
                    icon: dt.icon,
                    score: 92,
                    data: { value: dt.value },
                });
            }
        }
    }

    // Timer / stopwatch
    if (config.timer?.enabled !== false) {
        const timerCmd = parseTimerCommand(query);
        if (timerCmd) {
            results.push({
                id: 'timer:' + timerCmd.type,
                type: 'timer_cmd',
                label: timerCmd.type === 'hint' ? 'Timer' : timerCmd.type === 'timer' ? 'Start Timer' : 'Stopwatch',
                description: timerCmd.type === 'hint' ? 'timer 5m · timer 1h30m · timer 90s' : 'Press Enter',
                icon: '⏱️',
                score: 91,
                data: timerCmd,
            });
        }
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
                results.push({
                    id: 'shortcut:' + sc.name,
                    type: 'shortcut',
                    label: sc.name,
                    description: '⚡ ' + sc.shortcut,
                    icon: '⚡',
                    score: scLower === lower || nameLower === lower ? 85 : 65,
                    data: { shortcut: sc, args: query },
                });
            }
        }
    }

    // --- Rust-side search (one IPC call for URL, path, well-known dir, system cmd, apps) ---
    try {
        const rustResult = await invoke('handle_floating_input', { input: query });
        if (rustResult.startsWith('url:')) {
            results.push({ id: 'url:' + rustResult, type: 'url', label: 'Open in browser', description: rustResult.substring(4), icon: '🌐', score: 88, data: { value: rustResult.substring(4) } });
        } else if (rustResult.startsWith('path:')) {
            const pathInfo = rustResult.substring(5);
            const colonIdx = pathInfo.indexOf(':');
            const pathType = pathInfo.substring(0, colonIdx);
            const path = pathInfo.substring(colonIdx + 1);
            results.push({ id: 'path:' + path, type: 'path', label: pathType === 'file' ? 'Open File' : 'Open Folder', description: path, icon: pathType === 'file' ? '📄' : '📁', score: 87, data: { value: path, pathType } });
        } else if (rustResult.startsWith('system:')) {
            const parts = rustResult.substring(7).split(':');
            results.push({ id: 'system:' + parts[0], type: 'system', label: parts[1], description: parts[2] === 'confirm' ? 'Press Enter to select' : 'Press Enter to execute', icon: '⚙️', score: 86, data: { cmdId: parts[0], cmdLabel: parts[1], needsConfirm: parts[2] === 'confirm' } });
        } else if (rustResult.startsWith('multiple:') || rustResult.startsWith('launched:')) {
            const apps = JSON.parse(rustResult.substring(rustResult.indexOf(':') + 1));
            for (let i = 0; i < apps.length; i++) {
                results.push({ id: 'app:' + apps[i].name, type: 'app', label: apps[i].name, description: '', icon: '', score: 80 - i, data: apps[i] });
            }
        }
        // 'chat' result = no match, don't add anything
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
            iconHtml = `<div class="app-icon" style="background:${hex};border:2px solid rgba(255,255,255,0.2)"></div>`;
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
    setTimeout(() => resizeWindow(), 10);
    return 0;
}

function _escapeHtml(str) {
    return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

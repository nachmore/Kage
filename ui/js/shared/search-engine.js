/**
 * Unified search engine — queries all sources in parallel, merges with frecency.
 * Shared across floating and chat windows.
 */

import { matchCommands, matchSlashCommands, matchCommandsByName } from './commands.js';

// --- Frecency store ---

let _frecencyData = {};
let _frecencyLoaded = false;

export function recordSelection(query, resultId, invoke) {
    const prefix = query.toLowerCase().substring(0, 6);
    if (!_frecencyData[resultId]) {
        _frecencyData[resultId] = { count: 0, lastUsed: 0, prefixes: {} };
    }
    const entry = _frecencyData[resultId];
    entry.count++;
    entry.lastUsed = Date.now();
    entry.prefixes[prefix] = (entry.prefixes[prefix] || 0) + 1;
    _debounceSave(invoke);
}

function getFrecencyBoost(query, resultId) {
    const entry = _frecencyData[resultId];
    if (!entry) return 0;
    const prefix = query.toLowerCase().substring(0, 6);
    const prefixCount = entry.prefixes[prefix] || 0;
    const totalCount = entry.count;
    const ageMs = Date.now() - entry.lastUsed;
    const ageDays = ageMs / 86400000;
    let recencyMultiplier = 1;
    if (ageDays > 90) recencyMultiplier = 0.1;
    else if (ageDays > 30) recencyMultiplier = 0.25;
    else if (ageDays > 7) recencyMultiplier = 0.5;
    return (prefixCount * 15 + totalCount * 3) * recencyMultiplier;
}

let _saveTimer = null;
function _debounceSave(invoke) {
    if (_saveTimer) clearTimeout(_saveTimer);
    _saveTimer = setTimeout(() => {
        const cutoff = Date.now() - 90 * 86400000;
        for (const [id, entry] of Object.entries(_frecencyData)) {
            if (entry.lastUsed < cutoff) delete _frecencyData[id];
        }
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

export function setExtensionManager(mgr) {
    _extensionManager = mgr;
}

export function getExtensionManager() {
    return _extensionManager;
}

// --- Unified search ---

export async function unifiedSearch(query, invoke, shortcuts) {
    if (!query) return [];

    const results = [];

    // Extension search providers
    if (_extensionManager) {
        const extResults = _extensionManager.matchAll(query);
        results.push(...extResults);
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
                const triggerLen = scLower.startsWith(lower) ? sc.shortcut.length : sc.name.length;
                const rawArgs = query.length > triggerLen ? query.substring(triggerLen).trim() : '';
                const argsArray = rawArgs ? rawArgs.split(/\s+/) : [];

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
                    icon: sc.icon || '⚡',
                    score: scLower === lower || nameLower === lower ? 85 : 65,
                    data: { shortcut: sc, args: argsArray },
                });
            }
        }
    }

    // Rust-side search (apps, URLs, paths, system commands)
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

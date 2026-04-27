/**
 * Unified search engine — queries all sources in parallel, merges with frecency.
 * Shared across floating and chat windows.
 */

import { matchCommands, matchSlashCommands, matchCommandsByName } from './commands.js';

// --- Helpers ---

function _relativeTime(isoString) {
    try {
        const d = new Date(isoString);
        const diffMin = Math.floor((Date.now() - d) / 60000);
        if (diffMin < 1) return 'just now';
        if (diffMin < 60) return `${diffMin}m ago`;
        const diffHr = Math.floor(diffMin / 60);
        if (diffHr < 24) return `${diffHr}h ago`;
        const diffDay = Math.floor(diffHr / 24);
        if (diffDay < 7) return `${diffDay}d ago`;
        return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
    } catch { return ''; }
}

function _fileIcon(ext) {
    const icons = {
        pdf: '📕', doc: '📘', docx: '📘', xls: '📗', xlsx: '📗', ppt: '📙', pptx: '📙',
        txt: '📄', md: '📄', csv: '📄', json: '📄', xml: '📄', yaml: '📄', yml: '📄',
        js: '📜', ts: '📜', py: '📜', rs: '📜', java: '📜', cpp: '📜', c: '📜', h: '📜',
        html: '🌐', css: '🎨', svg: '🎨',
        png: '🖼️', jpg: '🖼️', jpeg: '🖼️', gif: '🖼️', bmp: '🖼️', ico: '🖼️', webp: '🖼️',
        mp3: '🎵', wav: '🎵', flac: '🎵', ogg: '🎵', m4a: '🎵',
        mp4: '🎬', avi: '🎬', mkv: '🎬', mov: '🎬', wmv: '🎬',
        zip: '📦', rar: '📦', '7z': '📦', tar: '📦', gz: '📦',
        exe: '⚙️', msi: '⚙️', bat: '⚙️', cmd: '⚙️', ps1: '⚙️',
    };
    return icons[ext] || '📄';
}

function _formatSize(bytes) {
    if (!bytes || bytes === 0) return '';
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1048576) return (bytes / 1024).toFixed(1) + ' KB';
    if (bytes < 1073741824) return (bytes / 1048576).toFixed(1) + ' MB';
    return (bytes / 1073741824).toFixed(1) + ' GB';
}

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
        invoke('save_extension_data', { key: 'search-frecency', data: JSON.stringify(_frecencyData) }).catch(() => {});
    }, 2000);
}

export async function loadFrecency(invoke) {
    if (_frecencyLoaded) return;
    try {
        const json = await invoke('load_extension_data', { key: 'search-frecency' });
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

export async function unifiedSearch(query, invoke, shortcuts, onPartial) {
    if (!query) return [];

    const results = [];

    // --- Synchronous matchers (fast, no I/O) ---

    // Extension sync search providers
    // NOTE: sandbox RPC is async, so matchAll() is now a Promise even
    // though the extension's own match() is still synchronous.
    if (_extensionManager) {
        try {
            const extResults = await _extensionManager.matchAll(query);
            results.push(...extResults);
        } catch (e) {
            console.warn('extension matchAll failed:', e);
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
        // Don't return early for >find or >cb — let them fall through to file search / clipboard
        const lowerQuery = query.toLowerCase().trim();
        if (!lowerQuery.startsWith('>find ') && !lowerQuery.startsWith('>cb') && !lowerQuery.startsWith('>clipboard')) {
            return _applyFrecency(results, query);
        }
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

    // Shortcuts (sync matching + collect async history promises)
    const historyPromises = [];
    if (shortcuts && shortcuts.length > 0) {
        const lower = query.toLowerCase();
        const parts = query.split(/\s+/);
        const triggerWord = parts[0].toLowerCase();
        const hasSpace = query.includes(' ');
        const partialArgs = hasSpace ? parts.slice(1).join(' ').toLowerCase() : '';

        for (const sc of shortcuts) {
            const scLower = sc.shortcut?.toLowerCase() || '';
            const nameLower = sc.name?.toLowerCase() || '';
            const triggerMatch = scLower.startsWith(lower) || (hasSpace && scLower === triggerWord);
            const nameMatch = nameLower.startsWith(lower) || nameLower.includes(lower);
            if (triggerMatch || nameMatch) {
                const matchedByTrigger = scLower.startsWith(lower) || (hasSpace && scLower === triggerWord);
                const triggerLen = matchedByTrigger ? sc.shortcut.length : sc.name.length;
                const rawArgs = matchedByTrigger ? (query.length > triggerLen ? query.substring(triggerLen).trim() : '') : '';
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

                // Collect history fetch as a promise (resolved later in parallel)
                if (scLower === triggerWord && hasSpace) {
                    historyPromises.push(
                        invoke('get_shortcut_history', { trigger: sc.shortcut })
                            .then(history => {
                                const histResults = [];
                                for (const entry of history) {
                                    const histArgs = entry.args || '';
                                    if (partialArgs && !histArgs.toLowerCase().includes(partialArgs)) continue;
                                    if (rawArgs && histArgs === rawArgs) continue;
                                    histResults.push({
                                        id: 'shortcut-history:' + sc.name + ':' + histArgs,
                                        type: 'shortcut',
                                        label: sc.name + ' ' + histArgs,
                                        description: '🕐 ' + (entry.at ? _relativeTime(entry.at) : ''),
                                        icon: sc.icon || '⚡',
                                        score: 84,
                                        data: { shortcut: sc, args: histArgs.split(/\s+/) },
                                    });
                                }
                                return histResults;
                            })
                            .catch(() => [])
                    );
                }
            }
        }
    }

    // --- Flush sync results immediately if callback provided ---
    if (onPartial && results.length > 0) {
        onPartial(_applyFrecency([...results], query), { done: false });
    }

    // --- Async sources (fire in parallel, flush each as it resolves) ---

    const asyncTasks = []; // { name: string, promise: Promise<array> }

    // Extension async search
    if (_extensionManager) {
        asyncTasks.push({
            name: 'extensions',
            promise: _extensionManager.matchAllAsync(query).catch(() => [])
        });
    }

    // Rust-side search (apps, URLs, paths, system commands)
    asyncTasks.push({
        name: 'apps',
        promise: invoke('handle_floating_input', { input: query })
            .then(rustJson => {
                const rustResults = JSON.parse(rustJson);
                const mapped = [];
                for (const r of rustResults) {
                    if (r.type === 'url') {
                        mapped.push({ id: 'url:' + r.value, type: 'url', label: 'Open in browser', description: r.value, icon: '🌐', score: r.score || 88, data: { value: r.value } });
                    } else if (r.type === 'path') {
                        mapped.push({ id: 'path:' + r.value, type: 'path', label: r.pathType === 'file' ? 'Open File' : 'Open Folder', description: r.value, icon: r.pathType === 'file' ? '📄' : '📁', score: r.score || 87, data: { value: r.value, pathType: r.pathType } });
                    } else if (r.type === 'system') {
                        mapped.push({ id: 'system:' + r.cmdId, type: 'system', label: r.cmdLabel, description: r.needsConfirm ? 'Press Enter to select' : 'Press Enter to execute', icon: '⚙️', score: r.score || 86, data: { cmdId: r.cmdId, cmdLabel: r.cmdLabel, needsConfirm: r.needsConfirm } });
                    } else if (r.type === 'app') {
                        mapped.push({ id: 'app:' + r.name, type: 'app', label: r.name, description: '', icon: '', score: r.score || 75, tooltip: r.path || '', data: { name: r.name, icon_base64: r.icon_base64, emoji_icon: r.emoji_icon } });
                    }
                }
                return mapped;
            })
            .catch(() => [])
    });

    // File search — conditional
    const trimmedQuery = query.trim();
    const findPrefix = trimmedQuery.toLowerCase().startsWith('>find ');
    const hasExtension = /\.\w{1,6}$/.test(trimmedQuery) && !trimmedQuery.includes(' ');
    const hasWildcard = trimmedQuery.includes('*') || trimmedQuery.includes('?');
    if (findPrefix || hasExtension || hasWildcard) {
        const fileQuery = findPrefix
            ? trimmedQuery.replace(/^>?find\s+/i, '').trim()
            : trimmedQuery;
        if (fileQuery.length >= 2) {
            asyncTasks.push({
                name: 'searching files',
                promise: invoke('search_files', { query: fileQuery, maxResults: 8 })
                    .then(fileResults => {
                        const mapped = [];
                        for (const f of fileResults) {
                            const ext = f.name.includes('.') ? f.name.split('.').pop().toLowerCase() : '';
                            const icon = f.is_folder ? '📁' : _fileIcon(ext);
                            const sizeStr = f.is_folder ? '' : _formatSize(f.size);
                            const timeStr = f.modified ? _relativeTime(f.modified) : '';
                            const desc = [f.path, sizeStr, timeStr].filter(Boolean).join(' · ');
                            mapped.push({
                                id: 'file:' + f.path,
                                type: 'file',
                                label: f.name,
                                description: desc,
                                icon,
                                score: findPrefix ? 90 : 70,
                                data: { path: f.path, is_folder: f.is_folder },
                            });
                        }
                        return mapped;
                    })
                    .catch(e => { console.warn('[Search] File search failed:', e); return []; })
            });
        }
    }

    // History promises (unnamed — fast, not worth labeling)
    const historyEntries = historyPromises.map(p => ({ name: null, promise: p }));

    // Flush each async batch as it resolves (progressive rendering)
    if (onPartial) {
        const allEntries = [...asyncTasks, ...historyEntries];
        const allPromises = allEntries.map(e => e.promise);
        const pending = new Set(asyncTasks.map(t => t.name).filter(Boolean));

        for (const entry of allEntries) {
            entry.promise.then(batch => {
                if (entry.name) pending.delete(entry.name);
                if (Array.isArray(batch) && batch.length > 0) {
                    results.push(...batch);
                }
                const done = pending.size === 0;
                onPartial(_applyFrecency([...results], query), { done, pending: [...pending] });
            });
        }
        // Still await all for the final return value
        await Promise.all(allPromises);
    } else {
        // Legacy path: wait for everything, return once
        const allPromises = [...asyncTasks.map(t => t.promise), ...historyPromises];
        const allAsync = await Promise.all(allPromises);
        for (const batch of allAsync) {
            if (Array.isArray(batch)) results.push(...batch);
        }
    }

    return _applyFrecency(results, query);
}


function _applyFrecency(results, query) {
    for (const r of results) {
        r.score += getFrecencyBoost(query, r.id);
    }
    results.sort((a, b) => b.score - a.score);
    return results.slice(0, 12);
}

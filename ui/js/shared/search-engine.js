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
    } catch {
        return '';
    }
}

function _fileIcon(ext) {
    const icons = {
        pdf: '📕',
        doc: '📘',
        docx: '📘',
        xls: '📗',
        xlsx: '📗',
        ppt: '📙',
        pptx: '📙',
        txt: '📄',
        md: '📄',
        csv: '📄',
        json: '📄',
        xml: '📄',
        yaml: '📄',
        yml: '📄',
        js: '📜',
        ts: '📜',
        py: '📜',
        rs: '📜',
        java: '📜',
        cpp: '📜',
        c: '📜',
        h: '📜',
        html: '🌐',
        css: '🎨',
        svg: '🎨',
        png: '🖼️',
        jpg: '🖼️',
        jpeg: '🖼️',
        gif: '🖼️',
        bmp: '🖼️',
        ico: '🖼️',
        webp: '🖼️',
        mp3: '🎵',
        wav: '🎵',
        flac: '🎵',
        ogg: '🎵',
        m4a: '🎵',
        mp4: '🎬',
        avi: '🎬',
        mkv: '🎬',
        mov: '🎬',
        wmv: '🎬',
        zip: '📦',
        rar: '📦',
        '7z': '📦',
        tar: '📦',
        gz: '📦',
        exe: '⚙️',
        msi: '⚙️',
        bat: '⚙️',
        cmd: '⚙️',
        ps1: '⚙️',
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
        // Use the dedicated frecency commands rather than the generic
        // extension-data store. The latter is now namespaced per extension
        // (P0.3); this is host-level state, not extension state.
        invoke('save_frecency', { data: JSON.stringify(_frecencyData) }).catch((e) => {
            console.warn('[search] failed to save frecency data:', e);
        });
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

/**
 * Build keyword completion-hint rows for a query.
 *
 * A hint fires when the query is a *strict, incomplete* prefix of a
 * registered keyword (the keyword starts with the query but isn't equal to
 * it) and the query carries no space — once the user has typed a space the
 * keyword is committed and its own match() owns the row. An exact, complete
 * keyword match produces no hint, since the extension's match() already
 * surfaces the live result for it (a `cal` that's also a prefix of
 * `calendar` still hints `calendar`, just not `cal` itself).
 *
 * The hint row is `type: 'ext_keyword'`; selecting it fills the input with
 * the full keyword (+ a trailing space when the keyword takes arguments) via
 * the replace_input path, re-triggering search so the real rows appear.
 */
async function _keywordHints(query) {
    if (!_extensionManager?.getKeywordDefinitions) return [];
    const q = query.trim().toLowerCase();
    if (!q || q.includes(' ') || q.startsWith('>') || q.startsWith('/')) return [];

    const defs = await _extensionManager.getKeywordDefinitions();
    const hints = [];
    for (const d of defs) {
        if (d.keyword === q) continue; // complete — match() owns it
        if (!d.keyword.startsWith(q)) continue;
        hints.push({
            id: 'ext-keyword:' + d.extensionId + ':' + d.keyword,
            type: 'ext_keyword',
            label: d.label,
            description: d.description,
            icon: d.icon,
            // Slightly below a typical live extension row (85) so a real
            // result for an already-complete keyword outranks a hint for a
            // longer sibling, but above generic app/command rows.
            score: 78,
            data: {
                extensionId: d.extensionId,
                keyword: d.keyword,
                acceptsArgs: d.acceptsArgs,
                // What to put in the input on select. Trailing space when the
                // keyword takes args so the user can type them immediately.
                fill: d.acceptsArgs ? d.keyword + ' ' : d.keyword,
            },
        });
    }
    return hints;
}

// --- Query-shape heuristics ---

/**
 * Does this query look like a file search? File-shaped queries (a trailing
 * extension, a glob `*`/`?`, or an explicit `>find ` prefix) hit the disk,
 * so callers debounce them harder to avoid hammering the filesystem on
 * every keystroke. Pure predicate — exported for direct unit testing.
 *
 * @param {string} query
 * @returns {boolean}
 */
export function looksLikeFileSearch(query) {
    if (!query) return false;
    return (
        /\.\w{0,6}$/.test(query) ||
        query.includes('*') ||
        query.includes('?') ||
        query.toLowerCase().startsWith('>find ')
    );
}

/**
 * Debounce (ms) to apply before dispatching a search for `query`:
 * file-shaped queries wait longer (disk I/O), everything else is snappy.
 *
 * @param {string} query
 * @returns {number}
 */
export function searchDebounceMs(query) {
    return looksLikeFileSearch(query) ? 250 : 100;
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
            // Tag sync rows so a later matchAsync() batch from the same
            // extension can supersede them (see merge loop below). By
            // contract match() returns a placeholder/cached pass and
            // matchAsync() the real one; without this, a placeholder whose
            // id differs from the loaded row (e.g. focus-tracker's
            // `focus-loading-today` vs `focus-summary-today`) lingers in
            // the list. Extensions don't have to coordinate ids.
            for (const r of extResults) r._syncPlaceholder = true;
            results.push(...extResults);
        } catch (e) {
            console.warn('extension matchAll failed:', e);
        }

        // Keyword completion hints: when the query is an incomplete prefix
        // of a registered keyword (e.g. "cal-ref" → "cal-refresh"), surface a
        // hint row so the user sees the command exists before fully typing
        // it. Selecting it fills the input to the full keyword, at which
        // point the extension's own match() produces the real rows.
        try {
            const hints = await _keywordHints(query);
            results.push(...hints);
        } catch (e) {
            console.warn('extension keyword hints failed:', e);
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
        // Don't return early for >find / >cb / >p / >prompts — those fall through
        // to richer handling: file search, clipboard history, or the prompt browser.
        const lowerQuery = query.toLowerCase().trim();
        if (
            !lowerQuery.startsWith('>find ') &&
            !lowerQuery.startsWith('>cb') &&
            !lowerQuery.startsWith('>clipboard') &&
            lowerQuery !== '>p' &&
            !lowerQuery.startsWith('>p ') &&
            lowerQuery !== '>prompts' &&
            !lowerQuery.startsWith('>prompts ')
        ) {
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

    // > prompts / > p — dedicated browse view of prompt-type shortcuts.
    // Fired when the query matches `>p`, `>p <filter>`, `>prompts`, or
    // `>prompts <filter>`. Filters by trigger or name fragment after the
    // command word; an empty filter shows all prompt-type shortcuts.
    {
        const lowerQuery = query.toLowerCase().trim();
        const promptPrefixMatch = lowerQuery.match(/^>(p|prompts)(?:\s+(.*))?$/);
        if (promptPrefixMatch && shortcuts && shortcuts.length > 0) {
            // Drop the command-row results we collected above — `>p` is
            // a browse mode, not a command-typeahead. Keep an
            // explanatory header instead, so the user knows what they're
            // looking at.
            results.length = 0;
            const filter = (promptPrefixMatch[2] || '').toLowerCase().trim();
            const promptShortcuts = shortcuts
                .filter((sc) => (sc.action_type || 'run_program') === 'prompt')
                .filter((sc) => {
                    if (!filter) return true;
                    const trigger = (sc.shortcut || '').toLowerCase();
                    const name = (sc.name || '').toLowerCase();
                    const body = (sc.prompt || '').toLowerCase();
                    return (
                        trigger.includes(filter) || name.includes(filter) || body.includes(filter)
                    );
                });
            for (const sc of promptShortcuts) {
                // Two-line preview: name on top, trimmed prompt body
                // (single line, ellipsised) below. Helps the user
                // recognise prompts at a glance — the trigger word
                // alone isn't always meaningful six months in.
                const body = (sc.prompt || '').replace(/\s+/g, ' ').trim();
                const preview = body.length > 90 ? body.slice(0, 87) + '…' : body;
                results.push({
                    id: 'prompt:' + sc.shortcut,
                    type: 'shortcut',
                    label: sc.name || sc.shortcut,
                    description: '⚡ ' + sc.shortcut + ' · ' + preview,
                    icon: sc.icon || '💬',
                    score: 95,
                    data: { shortcut: sc, args: [] },
                });
            }
            // No fall-through: prompt browse owns this query.
            return _applyFrecency(results, query);
        }
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
                const matchedByTrigger =
                    scLower.startsWith(lower) || (hasSpace && scLower === triggerWord);
                const triggerLen = matchedByTrigger ? sc.shortcut.length : sc.name.length;
                const rawArgs = matchedByTrigger
                    ? query.length > triggerLen
                        ? query.substring(triggerLen).trim()
                        : ''
                    : '';
                const argsArray = rawArgs ? rawArgs.split(/\s+/) : [];

                let desc = '⚡ ' + sc.shortcut;
                const templates = [sc.url, sc.prompt, sc.arguments, sc.script]
                    .filter(Boolean)
                    .join(' ');
                const reqParams = new Set();
                let m;
                const re = /\{(\d+)\}(?!\?)/g;
                while ((m = re.exec(templates)) !== null) reqParams.add(parseInt(m[1], 10));
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
                            .then((history) => {
                                const histResults = [];
                                for (const entry of history) {
                                    const histArgs = entry.args || '';
                                    if (
                                        partialArgs &&
                                        !histArgs.toLowerCase().includes(partialArgs)
                                    )
                                        continue;
                                    if (rawArgs && histArgs === rawArgs) continue;
                                    histResults.push({
                                        id: 'shortcut-history:' + sc.name + ':' + histArgs,
                                        type: 'shortcut',
                                        label: sc.name + ' ' + histArgs,
                                        description:
                                            '🕐 ' + (entry.at ? _relativeTime(entry.at) : ''),
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
            promise: _extensionManager.matchAllAsync(query).catch(() => []),
        });
    }

    // Rust-side search (apps, URLs, paths, system commands)
    asyncTasks.push({
        name: 'apps',
        promise: invoke('handle_floating_input', { input: query })
            .then((rustJson) => {
                const rustResults = JSON.parse(rustJson);
                const mapped = [];
                for (const r of rustResults) {
                    if (r.type === 'url') {
                        mapped.push({
                            id: 'url:' + r.value,
                            type: 'url',
                            label: 'Open in browser',
                            description: r.value,
                            icon: '🌐',
                            score: r.score || 88,
                            data: { value: r.value },
                        });
                    } else if (r.type === 'path') {
                        mapped.push({
                            id: 'path:' + r.value,
                            type: 'path',
                            label: r.pathType === 'file' ? 'Open File' : 'Open Folder',
                            description: r.value,
                            icon: r.pathType === 'file' ? '📄' : '📁',
                            score: r.score || 87,
                            data: { value: r.value, pathType: r.pathType },
                        });
                    } else if (r.type === 'system') {
                        mapped.push({
                            id: 'system:' + r.cmdId,
                            type: 'system',
                            label: r.cmdLabel,
                            description: r.needsConfirm
                                ? 'Press Enter to select'
                                : 'Press Enter to execute',
                            icon: '⚙️',
                            score: r.score || 86,
                            data: {
                                cmdId: r.cmdId,
                                cmdLabel: r.cmdLabel,
                                needsConfirm: r.needsConfirm,
                            },
                        });
                    } else if (r.type === 'app') {
                        mapped.push({
                            id: 'app:' + r.name,
                            type: 'app',
                            label: r.name,
                            description: '',
                            icon: '',
                            score: r.score || 75,
                            tooltip: r.path || '',
                            data: {
                                name: r.name,
                                icon_base64: r.icon_base64,
                                emoji_icon: r.emoji_icon,
                            },
                        });
                    }
                }
                return mapped;
            })
            .catch(() => []),
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
                    .then((fileResults) => {
                        const mapped = [];
                        for (const f of fileResults) {
                            const ext = f.name.includes('.')
                                ? f.name.split('.').pop().toLowerCase()
                                : '';
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
                    .catch((e) => {
                        console.warn('[Search] File search failed:', e);
                        return [];
                    }),
            });
        }
    }

    // History promises (unnamed — fast, not worth labeling)
    const historyEntries = historyPromises.map((p) => ({ name: null, promise: p }));

    // Flush each async batch as it resolves (progressive rendering)
    if (onPartial) {
        const allEntries = [...asyncTasks, ...historyEntries];
        const allPromises = allEntries.map((e) => e.promise);
        const pending = new Set(asyncTasks.map((t) => t.name).filter(Boolean));

        for (const entry of allEntries) {
            entry.promise.then((batch) => {
                if (entry.name) pending.delete(entry.name);
                if (Array.isArray(batch) && batch.length > 0) {
                    _mergeAsyncBatch(results, batch);
                }
                const done = pending.size === 0;
                onPartial(_applyFrecency([...results], query), { done, pending: [...pending] });
            });
        }
        // Still await all for the final return value
        await Promise.all(allPromises);
    } else {
        // Legacy path: wait for everything, return once
        const allPromises = [...asyncTasks.map((t) => t.promise), ...historyPromises];
        const allAsync = await Promise.all(allPromises);
        for (const batch of allAsync) {
            if (Array.isArray(batch)) _mergeAsyncBatch(results, batch);
        }
    }

    return _applyFrecency(results, query);
}

/**
 * Merge an async result batch into the accumulated results.
 *
 * When an extension's matchAsync() returns rows, they supersede that
 * extension's sync match() placeholders — the loaded data replaces the
 * "Loading…" row even when the two carry different ids. Extensions that
 * return [] from matchAsync() (cache hit) leave their placeholder intact.
 * Non-extension batches (apps, files, history) are appended as-is.
 */
function _mergeAsyncBatch(results, batch) {
    const supersededExtIds = new Set();
    for (const r of batch) {
        if (r?._extensionId) supersededExtIds.add(r._extensionId);
    }
    if (supersededExtIds.size > 0) {
        for (let i = results.length - 1; i >= 0; i--) {
            const r = results[i];
            if (r._syncPlaceholder && supersededExtIds.has(r._extensionId)) {
                results.splice(i, 1);
            }
        }
    }
    results.push(...batch);
}

function _applyFrecency(results, query) {
    for (const r of results) {
        r.score += getFrecencyBoost(query, r.id);
    }
    results.sort((a, b) => b.score - a.score);
    return results.slice(0, 12);
}

/**
 * Command registry for > prefix (local) and / prefix (ACP slash) commands.
 */

import { platformKeyLabel } from './shortcuts.js';

const LOCAL_COMMANDS = [
    {
        name: 'settings',
        description: 'Open settings window',
        icon: '⚙️',
        execute: async (invoke, appWindow) => {
            await invoke('open_settings_window');
            await appWindow.hide();
        }
    },
    {
        name: 'quit',
        description: 'Quit Kage',
        icon: '🚪',
        execute: async (invoke) => {
            await invoke('quit_app');
        }
    },
    {
        name: 'restart',
        description: 'Restart Kage',
        icon: '🔄',
        execute: async (invoke) => {
            await invoke('restart_app');
        }
    },
    {
        name: 'inspect',
        description: 'Open developer tools',
        icon: '🔍',
        execute: async (invoke) => {
            await invoke('open_devtools');
        }
    },
    {
        name: 'clear-ux',
        description: 'Clear the visible response (does not clear conversation history)',
        icon: '🧹',
        execute: async (_invoke, appWindow) => {
            document.dispatchEvent(new CustomEvent('kage-clear'));
        }
    },
    {
        name: 'sessions',
        description: 'Open full chat with sessions',
        icon: '💬',
        execute: async (invoke, appWindow) => {
            document.dispatchEvent(new CustomEvent('kage-clear'));
            await invoke('open_chat_window');
            await appWindow.hide();
        }
    },
    {
        name: 'session-id',
        description: 'Show current ACP session ID',
        icon: '🔑',
        execute: async (invoke) => {
            try {
                const id = await invoke('get_current_session_id');
                const text = id || 'No active session';
                document.dispatchEvent(new CustomEvent('kage-show-response', { detail: text }));
            } catch (e) {
                document.dispatchEvent(new CustomEvent('kage-show-response', { detail: 'Error: ' + e }));
            }
        }
    },
    {
        name: 'session-title',
        description: 'Rename current session (>session-title My Project Chat)',
        icon: '✏️',
        execute: async (invoke, _appWindow, args) => {
            const title = args?.trim();
            if (!title) {
                document.dispatchEvent(new CustomEvent('kage-show-response', { detail: 'Usage: >session-title New Session Name' }));
                return;
            }
            try {
                const sessionId = await invoke('get_current_session_id');
                if (!sessionId) {
                    document.dispatchEvent(new CustomEvent('kage-show-response', { detail: 'No active session to rename' }));
                    return;
                }
                await invoke('rename_session', { sessionId, title });
                document.dispatchEvent(new CustomEvent('kage-show-response', { detail: `Session renamed to: ${title}` }));
            } catch (e) {
                document.dispatchEvent(new CustomEvent('kage-show-response', { detail: 'Error: ' + e }));
            }
        }
    },
    {
        name: 'store',
        description: 'Browse extensions, themes, and command packs',
        icon: '🛍️',
        execute: async (invoke, appWindow) => {
            await invoke('open_store_window');
            await appWindow.hide();
        }
    },
    {
        name: 'session-folder',
        description: 'Open current session file in file explorer',
        icon: '📂',
        execute: async (invoke) => {
            try {
                const sessionId = await invoke('get_current_session_id');
                if (!sessionId) {
                    document.dispatchEvent(new CustomEvent('kage-show-response', { detail: 'No active session' }));
                    return;
                }
                await invoke('reveal_session_file', { sessionId });
            } catch (e) {
                document.dispatchEvent(new CustomEvent('kage-show-response', { detail: 'Error: ' + e }));
            }
        }
    },
    {
        name: 'find',
        description: 'Search files by name (e.g. >find report or >find *.docx)',
        icon: '🔎',
        execute: async () => {
            // No-op — file search is handled by the search engine when it sees ">find " prefix.
            // This entry exists so >find shows up in the command suggestions.
        }
    },
    {
        name: 'clipboard',
        description: 'Browse clipboard history',
        icon: '📋',
        aliases: ['cb'],
        execute: async () => {
            const input = document.querySelector('#floatingInput, #chatInput');
            if (input) {
                input.value = '>cb ';
                input.dispatchEvent(new Event('input', { bubbles: true }));
            }
        }
    },
    {
        name: 'logs',
        description: 'View application logs',
        icon: '📋',
        execute: async (invoke, appWindow) => {
            await invoke('open_settings_window', { section: 'about', subSection: 'logging' });
            await appWindow.hide();
        }
    }
];

/** Cached ACP slash commands */
let acpSlashCommands = [];

/** Load slash commands from the backend */
export async function loadSlashCommands(invoke) {
    try {
        acpSlashCommands = await invoke('get_slash_commands');
        console.log('Loaded ACP slash commands:', acpSlashCommands);
    } catch (e) {
        console.log('No slash commands available yet:', e);
        acpSlashCommands = [];
    }
}

/** Get icon for a slash command based on its name */
function getSlashIcon(name) {
    const n = name.toLowerCase();
    if (n.includes('agent')) return '🤖';
    if (n.includes('model')) return '🧠';
    if (n.includes('clear')) return '🧹';
    if (n.includes('compact')) return '📦';
    if (n.includes('context')) return '📊';
    if (n.includes('help')) return '❓';
    if (n.includes('quit')) return '🚪';
    return '⚡';
}

/**
 * Check if input is a > command and return matching commands.
 */
export function matchCommands(input) {
    const trimmed = input.trim();
    if (!trimmed.startsWith('>')) return null;

    const query = trimmed.substring(1).trim().toLowerCase();
    if (query.length === 0) return [...LOCAL_COMMANDS];
    return LOCAL_COMMANDS.filter(cmd =>
        cmd.name.startsWith(query) || cmd.aliases?.some(a => a.startsWith(query))
    );
}

/**
 * Match commands by name without requiring the > prefix.
 * Used to show command suggestions inline alongside other results.
 */
export function matchCommandsByName(query) {
    const q = query.trim().toLowerCase();
    if (q.length === 0) return [];
    return LOCAL_COMMANDS
        .filter(cmd => cmd.name.startsWith(q) || cmd.aliases?.some(a => a.startsWith(q)))
        .map(cmd => ({ type: 'command', ...cmd }));
}

/**
 * Check if input is a / slash command and return matching commands.
 */
export function matchSlashCommands(input) {
    const trimmed = input.trim();
    if (!trimmed.startsWith('/')) return null;

    const query = trimmed.substring(1).trim().toLowerCase();

    // Map ACP commands to the display format
    const mapped = acpSlashCommands
        .filter(cmd => {
            // Skip /quit if it's marked as local — we handle that with >quit
            if (cmd.meta?.local) return false;
            if (query.length === 0) return true;
            return cmd.name.substring(1).toLowerCase().startsWith(query);
        })
        .map(cmd => ({
            type: 'slash',
            name: cmd.name,
            description: cmd.description + (cmd.meta?.hint ? ` (${cmd.meta.hint})` : ''),
            icon: getSlashIcon(cmd.name),
            meta: cmd.meta,
            execute: async (invoke, appWindow) => {
                const cmdName = cmd.name.startsWith('/') ? cmd.name.substring(1) : cmd.name;

                // Selection-type commands: show options as a selectable list
                if (cmd.meta?.inputType === 'selection') {
                    try {
                        const result = await invoke('execute_slash_command', {
                            command: cmdName,
                            args: null
                        });
                        // Parse the message to extract options
                        const msg = result?.message || '';
                        const lines = msg.split('\n').filter(l => l.trim());
                        if (lines.length > 0) {
                            document.dispatchEvent(new CustomEvent('kage-show-selection', {
                                detail: {
                                    command: cmdName,
                                    options: lines.map(line => {
                                        const isCurrent = line.trim().startsWith('→') || line.trim().startsWith('*');
                                        const clean = line.replace(/^[\s→*]+/, '').trim();
                                        // Extract name and id: "name (id)" or just "name"
                                        const match = clean.match(/^(.+?)\s*\(([^)]+)\)\s*$/);
                                        return {
                                            label: match ? match[1].trim() : clean,
                                            value: match ? match[2].trim() : clean,
                                            current: isCurrent
                                        };
                                    })
                                }
                            }));
                        } else {
                            document.dispatchEvent(new CustomEvent('kage-show-response', { detail: msg || 'No options available' }));
                        }
                    } catch (e) {
                        document.dispatchEvent(new CustomEvent('kage-show-response', { detail: 'Error: ' + e }));
                    }
                    return;
                }

                // Regular commands: execute and show result
                try {
                    const result = await invoke('execute_slash_command', {
                        command: cmdName,
                        args: null
                    });
                    const msg = result?.message || (result?.data ? JSON.stringify(result.data, null, 2) : 'Command executed');
                    document.dispatchEvent(new CustomEvent('kage-show-response', { detail: result?.message || msg }));
                } catch (e) {
                    document.dispatchEvent(new CustomEvent('kage-show-response', { detail: 'Error: ' + e }));
                }
            }
        }));

    return mapped.length > 0 ? mapped : null;
}

/**
 * Render command suggestions into the suggestions container.
 * Works for both > local commands and / slash commands.
 */
export function renderCommandSuggestions(commands, container, selectedIndex, onExecute, onResize, showSendHint = false) {
    container.innerHTML = '';

    commands.forEach((cmd, index) => {
        const item = document.createElement('div');
        item.className = 'app-suggestion-item' + (index === selectedIndex ? ' selected' : '');
        const prefix = cmd.type === 'slash' ? '' : '&gt; ';
        item.innerHTML = `
            <div class="app-icon">${cmd.icon}</div>
            <div class="app-info">
                <div class="app-name">${prefix}${cmd.name}</div>
                <div class="app-description">${cmd.description}</div>
            </div>
        `;
        item.addEventListener('click', () => onExecute(cmd));
        container.appendChild(item);
    });

    if (showSendHint) {
        const hint = document.createElement('div');
        hint.className = 'suggestions-hint';
        hint.innerHTML = `<span class="hint-key">${platformKeyLabel('Ctrl+Enter')}</span> to send to agent`;
        container.appendChild(hint);
    }

    container.classList.add('visible');
    if (onResize) setTimeout(onResize, 10);

    return Math.max(0, Math.min(selectedIndex, commands.length - 1));
}

/**
 * Execute a > command by name. Returns true if found and executed.
 */
export async function executeCommand(name, invoke, appWindow) {
    const parts = name.toLowerCase().split(/\s+/);
    const cmdName = parts[0];
    const args = parts.length > 1 ? name.substring(name.indexOf(' ') + 1) : '';
    const cmd = LOCAL_COMMANDS.find(c => c.name === cmdName);
    if (!cmd) return false;
    await cmd.execute(invoke, appWindow, args);
    return true;
}

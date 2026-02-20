/**
 * Command registry for > prefix (local) and / prefix (ACP slash) commands.
 */

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
        description: 'Quit Kiro Assistant',
        icon: '🚪',
        execute: async (invoke) => {
            await invoke('quit_app');
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
        name: 'clear',
        description: 'Clear current response',
        icon: '🧹',
        execute: async (_invoke, appWindow) => {
            document.dispatchEvent(new CustomEvent('kiro-clear'));
        }
    },
    {
        name: 'sessions',
        description: 'Open full chat with sessions',
        icon: '💬',
        execute: async (invoke, appWindow) => {
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
                document.dispatchEvent(new CustomEvent('kiro-show-response', { detail: text }));
            } catch (e) {
                document.dispatchEvent(new CustomEvent('kiro-show-response', { detail: 'Error: ' + e }));
            }
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
    return LOCAL_COMMANDS.filter(cmd => cmd.name.startsWith(query));
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
            description: cmd.description,
            icon: getSlashIcon(cmd.name),
            meta: cmd.meta,
            execute: async (invoke, appWindow) => {
                // Execute via ACP — command is a tagged enum: { command: "name", args: {} }
                const cmdName = cmd.name.startsWith('/') ? cmd.name.substring(1) : cmd.name;
                try {
                    const result = await invoke('execute_slash_command', {
                        command: cmdName,
                        args: null
                    });
                    // Show the result message in the floating window
                    const msg = result?.message || result?.data ? JSON.stringify(result.data, null, 2) : 'Command executed';
                    document.dispatchEvent(new CustomEvent('kiro-show-response', { detail: result?.message || msg }));
                } catch (e) {
                    document.dispatchEvent(new CustomEvent('kiro-show-response', { detail: 'Error: ' + e }));
                }
            }
        }));

    return mapped.length > 0 ? mapped : null;
}

/**
 * Render command suggestions into the suggestions container.
 * Works for both > local commands and / slash commands.
 */
export function renderCommandSuggestions(commands, container, selectedIndex, onExecute, onResize) {
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

    container.classList.add('visible');
    if (onResize) setTimeout(onResize, 10);

    return Math.max(0, Math.min(selectedIndex, commands.length - 1));
}

/**
 * Execute a > command by name. Returns true if found and executed.
 */
export async function executeCommand(name, invoke, appWindow) {
    const cmd = LOCAL_COMMANDS.find(c => c.name === name.toLowerCase());
    if (!cmd) return false;
    await cmd.execute(invoke, appWindow);
    return true;
}

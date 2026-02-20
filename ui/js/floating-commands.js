/**
 * Command registry for > prefix commands.
 * Easy to extend — just add entries to the COMMANDS array.
 */

const COMMANDS = [
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
    }
];

/**
 * Check if input is a > command and return matching commands.
 * Returns null if input doesn't start with >.
 */
export function matchCommands(input) {
    const trimmed = input.trim();
    if (!trimmed.startsWith('>')) return null;

    const query = trimmed.substring(1).trim().toLowerCase();

    // Empty query after > — show all commands
    if (query.length === 0) return [...COMMANDS];

    // Filter by prefix match
    return COMMANDS.filter(cmd => cmd.name.startsWith(query));
}

/**
 * Render command suggestions into the suggestions container.
 */
export function renderCommandSuggestions(commands, container, selectedIndex, onExecute, onResize) {
    container.innerHTML = '';

    commands.forEach((cmd, index) => {
        const item = document.createElement('div');
        item.className = 'app-suggestion-item' + (index === selectedIndex ? ' selected' : '');
        item.innerHTML = `
            <div class="app-icon">${cmd.icon}</div>
            <div class="app-info">
                <div class="app-name">&gt; ${cmd.name}</div>
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
 * Execute a command by name. Returns true if found and executed.
 */
export async function executeCommand(name, invoke, appWindow) {
    const cmd = COMMANDS.find(c => c.name === name.toLowerCase());
    if (!cmd) return false;
    await cmd.execute(invoke, appWindow);
    return true;
}

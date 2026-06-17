/**
 * Command registry for > prefix (local) and / prefix (ACP slash) commands.
 */

import { platformKeyLabel } from './shortcuts.js';
import { WINDOW } from './window-labels.js';
import { getWindowSessionOrNull } from './session-resolve.js';
import { errLabel } from './error-message.js';
import { t } from './i18n.js';
import { loadSelection } from './slash-selection.js';

// Each entry's `description` is a getter so it resolves through the active
// i18n catalog at the moment the launcher renders, not at module-import time
// (which would freeze the EN strings into the binary).
const LOCAL_COMMANDS = [
    {
        name: 'settings',
        get description() {
            return t('command.settings.description');
        },
        icon: '⚙️',
        execute: async (invoke, appWindow) => {
            await invoke('open_settings_window');
            await appWindow.hide();
        },
    },
    {
        name: 'quit',
        get description() {
            return t('command.quit.description');
        },
        icon: '🚪',
        execute: async (invoke) => {
            await invoke('quit_app');
        },
    },
    {
        name: 'restart',
        get description() {
            return t('command.restart.description');
        },
        icon: '🔄',
        execute: async (invoke) => {
            await invoke('restart_app');
        },
    },
    {
        name: 'inspect',
        get description() {
            return t('command.inspect.description');
        },
        icon: '🔍',
        execute: async (invoke) => {
            await invoke('open_devtools');
        },
    },
    {
        name: 'clear-ux',
        get description() {
            return t('command.clear_ux.description');
        },
        icon: '🧹',
        execute: async (_invoke, _appWindow) => {
            document.dispatchEvent(new CustomEvent('kage-clear'));
        },
    },
    {
        name: 'chats',
        get description() {
            return t('command.chats.description');
        },
        icon: '💬',
        execute: async (invoke, appWindow) => {
            document.dispatchEvent(new CustomEvent('kage-clear'));
            await invoke('open_chat_window');
            await appWindow.hide();
        },
    },
    {
        name: 'session-id',
        get description() {
            return t('command.session_id.description');
        },
        icon: '🔑',
        execute: async (invoke) => {
            try {
                const id = await invoke('get_window_session', { label: WINDOW.MAIN });
                const text = id || t('command.session_id.no_session');
                document.dispatchEvent(new CustomEvent('kage-show-response', { detail: text }));
            } catch (e) {
                document.dispatchEvent(
                    new CustomEvent('kage-show-response', {
                        detail: errLabel(t('command.error.label'), e),
                    })
                );
            }
        },
    },
    {
        name: 'session-title',
        get description() {
            return t('command.session_title.description');
        },
        icon: '✏️',
        execute: async (invoke, _appWindow, args) => {
            const title = args?.trim();
            if (!title) {
                document.dispatchEvent(
                    new CustomEvent('kage-show-response', {
                        detail: t('command.session_title.usage'),
                    })
                );
                return;
            }
            try {
                const sessionId = await invoke('get_window_session', { label: WINDOW.MAIN });
                if (!sessionId) {
                    document.dispatchEvent(
                        new CustomEvent('kage-show-response', {
                            detail: t('command.session_title.no_session'),
                        })
                    );
                    return;
                }
                await invoke('rename_session', { sessionId, title });
                document.dispatchEvent(
                    new CustomEvent('kage-show-response', {
                        detail: t('command.session_title.renamed', { title }),
                    })
                );
            } catch (e) {
                document.dispatchEvent(
                    new CustomEvent('kage-show-response', {
                        detail: errLabel(t('command.error.label'), e),
                    })
                );
            }
        },
    },
    {
        name: 'store',
        get description() {
            return t('command.store.description');
        },
        icon: '🛍️',
        execute: async (invoke, appWindow) => {
            await invoke('open_store_window');
            await appWindow.hide();
        },
    },
    {
        name: 'session-folder',
        get description() {
            return t('command.session_folder.description');
        },
        icon: '📂',
        execute: async (invoke) => {
            try {
                const sessionId = await invoke('get_window_session', { label: WINDOW.MAIN });
                if (!sessionId) {
                    document.dispatchEvent(
                        new CustomEvent('kage-show-response', {
                            detail: t('command.session_folder.no_session'),
                        })
                    );
                    return;
                }
                await invoke('reveal_session_file', { sessionId });
            } catch (e) {
                document.dispatchEvent(
                    new CustomEvent('kage-show-response', {
                        detail: errLabel(t('command.error.label'), e),
                    })
                );
            }
        },
    },
    {
        name: 'find',
        get description() {
            return t('command.find.description');
        },
        icon: '🔎',
        execute: async () => {
            // No-op — file search is handled by the search engine when it sees ">find " prefix.
            // This entry exists so >find shows up in the command suggestions.
        },
    },
    {
        name: 'clipboard',
        get description() {
            return t('command.clipboard.description');
        },
        icon: '📋',
        aliases: ['cb'],
        execute: async () => {
            const input = document.querySelector('#floatingInput, #chatInput');
            if (input) {
                input.value = '>cb ';
                input.dispatchEvent(new Event('input', { bubbles: true }));
            }
        },
    },
    {
        name: 'prompts',
        get description() {
            return t('command.prompts.description');
        },
        icon: '💬',
        aliases: ['p'],
        execute: async () => {
            // The search engine special-cases `>p` / `>prompts` to
            // render prompt-type Quick Commands. Selecting this entry
            // from the autocomplete just nudges the input so the user
            // lands in browse mode without typing the trailing space.
            // Match floating's #promptInput first, chat's #chatInput
            // second — only one is in scope per window.
            const input =
                document.querySelector('#promptInput') || document.querySelector('#chatInput');
            if (input) {
                input.value = '>p ';
                input.dispatchEvent(new Event('input', { bubbles: true }));
            }
        },
    },
    {
        name: 'logs',
        get description() {
            return t('command.logs.description');
        },
        icon: '📋',
        execute: async (invoke, appWindow) => {
            await invoke('open_settings_window', { section: 'about', subSection: 'logging' });
            await appWindow.hide();
        },
    },
    {
        name: 'version',
        get description() {
            return t('command.version.description');
        },
        icon: 'ℹ️',
        execute: async (invoke, appWindow) => {
            const show = (text) =>
                document.dispatchEvent(new CustomEvent('kage-show-response', { detail: text }));
            let info = {};
            let channel = 'stable';
            try {
                info = await invoke('get_app_info');
            } catch (e) {
                show(errLabel(t('command.version.error_app_info'), e));
                return;
            }
            try {
                const urls = await invoke('get_update_urls');
                channel = urls?.channel || 'stable';
            } catch {
                // Non-fatal — fall back to "stable" label.
            }

            const header = `**Kage v${info.version || '?'}**\n\n- Channel: \`${channel}\`\n\n`;
            show(header + t('command.version.checking'));

            try {
                const result = await invoke('check_for_update');
                if (result?.available_version) {
                    // `check_for_update` already emits `update_available`
                    // server-side, which the floating window listens for
                    // and raises its install banner. Tailor the response
                    // text to the calling window: floating gets a
                    // "click the banner above" hint; chat (no banner
                    // listener) gets a direct settings link.
                    const inFloating = appWindow?.label === WINDOW.FLOATING;
                    const cta = inFloating
                        ? t('command.version.cta_floating')
                        : t('command.version.cta_chat');
                    show(
                        header +
                            t('command.version.update_available', {
                                version: result.available_version,
                            }) +
                            '\n\n' +
                            cta
                    );
                } else {
                    show(
                        header +
                            t('command.version.up_to_date', {
                                version: result?.current_version || info.version || '?',
                            })
                    );
                }
            } catch (e) {
                const msg = e && typeof e === 'object' ? e.message || JSON.stringify(e) : String(e);
                show(header + t('command.version.check_failed', { message: msg }));
            }
        },
    },
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
    return LOCAL_COMMANDS.filter(
        (cmd) => cmd.name.startsWith(query) || cmd.aliases?.some((a) => a.startsWith(query))
    );
}

/**
 * Match commands by name without requiring the > prefix.
 * Used to show command suggestions inline alongside other results.
 */
export function matchCommandsByName(query) {
    const q = query.trim().toLowerCase();
    if (q.length === 0) return [];
    return LOCAL_COMMANDS.filter(
        (cmd) => cmd.name.startsWith(q) || cmd.aliases?.some((a) => a.startsWith(q))
    ).map((cmd) => ({ type: 'command', ...cmd }));
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
        .filter((cmd) => {
            // Skip /quit if it's marked as local — we handle that with >quit
            if (cmd.meta?.local) return false;
            if (query.length === 0) return true;
            return cmd.name.substring(1).toLowerCase().startsWith(query);
        })
        .map((cmd) => ({
            type: 'slash',
            name: cmd.name,
            description: cmd.description + (cmd.meta?.hint ? ` (${cmd.meta.hint})` : ''),
            icon: getSlashIcon(cmd.name),
            meta: cmd.meta,
            dispatch: cmd.dispatch || 'vendor',
            execute: async (invoke, _appWindow) => {
                const cmdName = cmd.name.startsWith('/') ? cmd.name.substring(1) : cmd.name;

                // Prompt-dispatch commands (standard ACP, e.g. Claude): the
                // agent interprets the slash text itself when sent as a normal
                // prompt, and the answer streams back as an assistant message.
                // Don't intercept — hand the slash text to the window's normal
                // streaming send via `kage-send-prompt`. See chat/floating
                // handlers.
                if ((cmd.dispatch || 'vendor') === 'prompt') {
                    document.dispatchEvent(
                        new CustomEvent('kage-send-prompt', { detail: { text: cmd.name } })
                    );
                    return;
                }

                // Slash commands need a session id; the calling window
                // tells us its label so we can look up its pinned session.
                const winLabel = _appWindow?.label || WINDOW.FLOATING;
                const sessionId = await getWindowSessionOrNull(invoke, winLabel);

                // Selection-type commands: load options from the structured
                // reply and show them as a selectable list. The shared
                // `loadSelection` classifies the reply — a real option list
                // ('options') vs a plain message ('message', e.g. /feedback
                // opening a browser or /effort erroring on an unsupported
                // model). See ui/js/shared/slash-selection.js.
                if (cmd.meta?.inputType === 'selection') {
                    try {
                        const res = await loadSelection(invoke, sessionId, cmdName);
                        if (res.kind === 'options') {
                            document.dispatchEvent(
                                new CustomEvent('kage-show-selection', {
                                    detail: {
                                        command: cmdName,
                                        options: res.options,
                                    },
                                })
                            );
                            // The picker was just rendered into the suggestions
                            // dropdown (floating) — tell the Enter handler NOT to
                            // wipe it as part of its post-execute input cleanup.
                            // See handleEnterAction / floating handleEnterKey.
                            return { action: 'keep_suggestions' };
                        }
                        document.dispatchEvent(
                            new CustomEvent('kage-show-response', {
                                detail: res.text || t('command.slash.no_options'),
                            })
                        );
                    } catch (e) {
                        document.dispatchEvent(
                            new CustomEvent('kage-show-response', {
                                detail: errLabel(t('command.error.label'), e),
                            })
                        );
                    }
                    return;
                }

                // Regular commands: execute and show result
                try {
                    const result = await invoke('execute_slash_command', {
                        sessionId,
                        command: cmdName,
                        args: null,
                    });
                    // Prefer the agent-specific prettified markdown
                    // (`displayMessage`, attached by the Rust slash_format
                    // layer) over the plain one-line `message`. Falls back to
                    // the raw data dump only when neither is present.
                    const msg =
                        result?.displayMessage ||
                        result?.message ||
                        (result?.data
                            ? JSON.stringify(result.data, null, 2)
                            : t('command.slash.executed'));
                    document.dispatchEvent(new CustomEvent('kage-show-response', { detail: msg }));
                } catch (e) {
                    document.dispatchEvent(
                        new CustomEvent('kage-show-response', {
                            detail: errLabel(t('command.error.label'), e),
                        })
                    );
                }
            },
        }));

    return mapped.length > 0 ? mapped : null;
}

/**
 * Look up the advertised meta for a bare slash command name (no leading `/`).
 * Returns null if the agent never advertised it. Used by callers that need to
 * branch on `inputType` (selection vs panel vs plain) before executing — e.g.
 * the chat window's direct "type /agent and press Enter" path.
 */
export function getSlashCommandMeta(name) {
    const bare = name.startsWith('/') ? name.substring(1) : name;
    const cmd = acpSlashCommands.find(
        (c) => (c.name.startsWith('/') ? c.name.substring(1) : c.name) === bare
    );
    return cmd?.meta || null;
}

/**
 * Dispatch mode for a bare slash command name: "prompt" (standard ACP — send
 * as a normal message) or "vendor" (Kiro — vendor commands/execute RPC).
 * Defaults to "vendor" for unknown commands so callers keep the existing path.
 */
export function getSlashCommandDispatch(name) {
    const bare = name.startsWith('/') ? name.substring(1) : name;
    const cmd = acpSlashCommands.find(
        (c) => (c.name.startsWith('/') ? c.name.substring(1) : c.name) === bare
    );
    return cmd?.dispatch || 'vendor';
}

/**
 * Render command suggestions into the suggestions container.
 * Works for both > local commands and / slash commands.
 */
export function renderCommandSuggestions(
    commands,
    container,
    selectedIndex,
    onExecute,
    onResize,
    showSendHint = false
) {
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
        const keyHtml = `<span class="hint-key">${platformKeyLabel('Ctrl+Enter')}</span>`;
        hint.innerHTML = t('command.suggestions.send_hint', { key: keyHtml });
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
    const cmd = LOCAL_COMMANDS.find((c) => c.name === cmdName);
    if (!cmd) return false;
    await cmd.execute(invoke, appWindow, args);
    return true;
}

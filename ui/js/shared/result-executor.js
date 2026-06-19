/**
 * Shared result execution — dispatches unified search results to the appropriate action.
 * Used by both floating and chat windows.
 *
 * @param {Object} result - A unified search result
 * @param {Object} ctx - Execution context with: invoke, extensionManager, clipboard, callbacks
 * @returns {Promise<{handled: boolean, action?: string}>}
 */

import { buildShortcutCommand } from './shortcuts.js';
import { kageLog } from './kage-log.js';
import { executeCommand } from './commands.js';
import { recordSelection } from './search-engine.js';

/**
 * Execute a unified search result.
 * @param {Object} result - The search result to execute
 * @param {string} query - The original query text
 * @param {Object} ctx - Context object:
 *   - invoke: Tauri invoke function
 *   - appWindow: Tauri window reference
 *   - extensionManager: ExtensionManager instance (optional)
 *   - selectionText: selected text for shortcut substitution (optional)
 *   - onPrompt: async (text) => {} — called when result wants to send a prompt
 *   - onDisplay: (text) => {} — called when result wants to display text
 *   - onCopy: async (text) => {} — called when result wants to copy text
 *   - onTimerStart: (durationMs) => {} — called for timer commands (optional)
 *   - onStopwatch: () => {} — called for stopwatch commands (optional)
 */
export async function executeResult(result, query, ctx) {
    const { invoke, appWindow, extensionManager } = ctx;

    // Record frecency
    if (result.id) recordSelection(query, result.id, invoke);

    // Keyword completion hint — not a real action. Fill the input with the
    // full keyword (the search engine put the fill text in data.fill) and let
    // the re-triggered search surface the extension's real rows. This row
    // carries no _extensionId, so it must be handled before the delegation
    // branch below.
    if (result.type === 'ext_keyword') {
        const fill = result.data?.fill ?? result.data?.keyword ?? '';
        if (fill && ctx.onReplaceInput) {
            ctx.onReplaceInput(fill);
            return { handled: true, action: 'replace_input' };
        }
        return { handled: true };
    }

    // Extension-provided results — delegate to extension
    if (result._extensionId && extensionManager) {
        const action = await extensionManager.executeResult(result);
        if (action) {
            if (action.type === 'copy') {
                if (ctx.onCopy) await ctx.onCopy(action.value);
                else {
                    try {
                        await navigator.clipboard.writeText(action.value);
                    } catch {}
                }
            } else if (action.type === 'prompt' && ctx.onPrompt) {
                await ctx.onPrompt(action.value);
            } else if (action.type === 'display' && ctx.onDisplay) {
                ctx.onDisplay(action.value);
            } else if (action.type === 'replace_input' && ctx.onReplaceInput) {
                ctx.onReplaceInput(action.value);
                return { handled: true, action: 'replace_input' };
            }
        }
        // Timer/stopwatch special handling
        if (result.type === 'timer_cmd' && result.data) {
            if (result.data.type === 'timer' && result.data.durationMs && ctx.onTimerStart) {
                ctx.onTimerStart(result.data.durationMs);
            } else if (result.data.type === 'stopwatch' && ctx.onStopwatch) {
                ctx.onStopwatch();
            }
        }
        return { handled: true };
    }

    // Commands
    if (result.type === 'command' || result.type === 'slash') {
        const cmd = result.data || result;
        if (cmd.execute) {
            // A selection-type slash command renders a picker into the
            // suggestions dropdown and returns { action: 'keep_suggestions' }
            // so the caller doesn't wipe it during post-execute cleanup.
            const out = await cmd.execute(invoke, appWindow);
            if (out?.action === 'keep_suggestions') {
                return { handled: true, action: 'keep_suggestions' };
            }
        } else if (cmd.name) {
            await executeCommand(cmd.name, invoke, appWindow);
        }
        return { handled: true };
    }

    // URL
    if (result.type === 'url') {
        await invoke('open_url', { url: result.data?.value || result.value });
        return { handled: true };
    }

    // Path
    if (result.type === 'path') {
        await invoke('open_path', { path: result.data?.value || result.value });
        return { handled: true };
    }

    // App launch
    if (result.type === 'app') {
        await invoke('launch_app_by_name', { appName: result.data?.name || result.label });
        return { handled: true };
    }

    // File search result — open the file or folder
    if (result.type === 'file') {
        const filePath = result.data?.path || result.description;
        if (filePath) {
            await invoke('open_path', { path: filePath });
            return { handled: true, action: 'hide' };
        }
    }

    // System command
    if (result.type === 'system') {
        const d = result.data || result;
        // System commands need confirmation flow — return unhandled so the app can manage it
        return { handled: false, action: 'system', data: d };
    }

    // Shortcut
    if (result.type === 'shortcut') {
        const sc = result.data?.shortcut || result.shortcut;
        const args = result.data?.args || result.args;
        const command = buildShortcutCommand(sc, args, ctx.selectionText || '');
        const execResult = await executeShortcutCommand(command, ctx);
        // Record usage for history (fire-and-forget)
        if (execResult.handled && args?.length > 0 && sc?.shortcut && ctx.invoke) {
            ctx.invoke('record_shortcut_usage', {
                trigger: sc.shortcut,
                args: args.join(' '),
            }).catch((e) => {
                console.warn('[Shortcuts] Failed to record usage:', e);
            });
        }
        return execResult;
    }

    return { handled: false };
}

/**
 * Execute a built shortcut command object.
 */
export async function executeShortcutCommand(command, ctx) {
    const { invoke } = ctx;

    if (command.type === 'error') {
        if (ctx.onDisplay) ctx.onDisplay(command.message);
        return { handled: true };
    }
    if (command.type === 'noop') {
        return { handled: true };
    }
    if (command.type === 'open_url') {
        kageLog.info('shortcuts', 'Open URL: ' + command.url);
        await invoke('open_url', { url: command.url });
        return { handled: true, action: 'hide' };
    }
    if (command.type === 'prompt' && ctx.onPrompt) {
        await ctx.onPrompt(command.message);
        return { handled: true };
    }
    if (command.type === 'prompt_form' && ctx.onPromptForm) {
        // Prompt-type quick command needs named placeholders the user
        // hasn't supplied positionally. Hand off to the launcher's
        // form UI; on submit it re-runs buildShortcutCommand with
        // paramsByName populated and re-enters this executor.
        await ctx.onPromptForm(command);
        return { handled: true };
    }
    if (command.type === 'text' && ctx.onDisplay) {
        ctx.onDisplay(command.message);
        return { handled: true };
    }
    if (command.type === 'run_program') {
        await invoke('execute_shortcut', {
            path: command.path,
            args: command.args,
            workingDirectory: command.workDir || null,
        });
        return { handled: true, action: 'hide' };
    }
    return { handled: false };
}

/**
 * Handle Enter key press — shared flow for both floating and chat windows.
 * Checks suggestions, then direct command/shortcut matching, then falls back to sending.
 *
 * @param {Object} opts
 * @param {string} opts.message - Trimmed input text
 * @param {Array} opts.suggestions - Current suggestion matches
 * @param {number} opts.selectedIndex - Currently selected suggestion index
 * @param {Array} opts.shortcuts - Loaded shortcut configs
 * @param {Object} opts.ctx - Execution context (from _getExecCtx)
 * @param {Function} opts.onSend - async (message) => {} — send to agent
 * @param {Function} [opts.onSystemCommand] - async (cmdId, needsConfirm, elevated) => {} — system command handler
 * @param {Function} [opts.onSelection] - async (command, value) => {} — selection list handler
 * @returns {Promise<{handled: boolean, action?: string}>}
 */
export async function handleEnterAction(opts) {
    const {
        message,
        suggestions,
        selectedIndex,
        shortcuts,
        ctx,
        onSend,
        onSystemCommand,
        onSelection,
    } = opts;

    // If a suggestion is selected, execute it
    if (suggestions.length > 0 && selectedIndex >= 0) {
        const selected = suggestions[selectedIndex];

        // System commands have a special confirmation flow (floating-only)
        if (selected.type === 'system' && onSystemCommand) {
            const d = selected.data || selected;
            await onSystemCommand(d.cmdId, d.needsConfirm, false);
            return { handled: true };
        }
        if (selected.type === 'system_confirm' && onSystemCommand) {
            const d = selected.data || selected;
            await onSystemCommand(d.cmdId, false, d.elevated || false);
            return { handled: true };
        }
        // Selection lists (floating-only)
        if (selected.type === 'selection' && onSelection) {
            await onSelection(
                selected.data?.command || selected.command,
                selected.data?.value || selected.value
            );
            return { handled: true };
        }

        // Everything else — extensions, commands, shortcuts, URLs, paths, apps
        const result = await executeResult(selected, message, ctx);
        if (result.handled) return result;
    }

    // No suggestion selected — try direct command/shortcut matching
    if (message.startsWith('>')) {
        const cmdName = message.substring(1).trim();
        const { executeCommand: execCmd } = await import('./commands.js');
        if (await execCmd(cmdName, ctx.invoke, ctx.appWindow)) {
            return { handled: true };
        }
    }
    if (message.startsWith('/')) {
        const { matchSlashCommands: matchSlash } = await import('./commands.js');
        const slashCmds = matchSlash(message);
        if (slashCmds?.length === 1) {
            const out = await slashCmds[0].execute(ctx.invoke, ctx.appWindow);
            if (out?.action === 'keep_suggestions') {
                return { handled: true, action: 'keep_suggestions' };
            }
            return { handled: true };
        }
    }

    // Try shortcut matching
    if (shortcuts?.length > 0) {
        const { matchShortcut: matchSc } = await import('./shortcuts.js');
        const matches = matchSc(message, shortcuts);
        if (matches?.length > 0) {
            const cmd = buildShortcutCommand(
                matches[0].shortcut,
                matches[0].args,
                ctx.selectionText || ''
            );
            const result = await executeShortcutCommand(cmd, ctx);
            // Record usage for history
            if (result.handled && matches[0].args?.length > 0 && ctx.invoke) {
                ctx.invoke('record_shortcut_usage', {
                    trigger: matches[0].shortcut.shortcut,
                    args: matches[0].args.join(' '),
                }).catch((e) => {
                    console.warn('[Shortcuts] Failed to record usage:', e);
                });
            }
            return result.handled ? result : { handled: false };
        }
    }

    // Nothing matched — send to agent
    await onSend(message);
    return { handled: true };
}

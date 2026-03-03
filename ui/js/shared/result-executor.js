/**
 * Shared result execution — dispatches unified search results to the appropriate action.
 * Used by both floating and chat windows.
 *
 * @param {Object} result - A unified search result
 * @param {Object} ctx - Execution context with: invoke, extensionManager, clipboard, callbacks
 * @returns {Promise<{handled: boolean, action?: string}>}
 */

import { buildShortcutCommand } from './shortcuts.js';
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

    // Extension-provided results — delegate to extension
    if (result._extensionId && extensionManager) {
        const action = extensionManager.executeResult(result);
        if (action) {
            if (action.type === 'copy') {
                if (ctx.onCopy) await ctx.onCopy(action.value);
                else { try { await navigator.clipboard.writeText(action.value); } catch {} }
            } else if (action.type === 'prompt' && ctx.onPrompt) {
                await ctx.onPrompt(action.value);
            } else if (action.type === 'display' && ctx.onDisplay) {
                ctx.onDisplay(action.value);
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
            await cmd.execute(invoke, appWindow);
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
        await invoke('launch_app', { name: result.data?.name || result.label });
        return { handled: true };
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
        return await executeShortcutCommand(command, ctx);
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
        await invoke('open_url', { url: command.url });
        return { handled: true, action: 'hide' };
    }
    if (command.type === 'prompt' && ctx.onPrompt) {
        await ctx.onPrompt(command.message);
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
            workingDirectory: command.workDir || null
        });
        return { handled: true, action: 'hide' };
    }
    return { handled: false };
}

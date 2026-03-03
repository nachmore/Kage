/**
 * Shortcut matching and command building — pure logic, no UI dependencies.
 * Reusable across floating and chat windows.
 *
 * Usage:
 *   import { matchShortcut, buildShortcutCommand } from './shortcuts.js';
 */

/**
 * Find shortcuts matching the input trigger word, scored by argument compatibility.
 * @param {string} input - Full user input string
 * @param {Array} shortcuts - Array of shortcut config objects
 * @returns {Array|null} Scored matches sorted by score (highest first), or null
 */
export function matchShortcut(input, shortcuts) {
    const parts = input.split(/\s+/);
    const trigger = parts[0].toLowerCase();
    const args = parts.slice(1);

    const matches = shortcuts.filter(s => s.shortcut.toLowerCase() === trigger);
    if (matches.length === 0) return null;

    const scoredMatches = matches.map(shortcut => {
        const score = scoreShortcutMatch(shortcut, args);
        return { shortcut, args, score };
    });

    scoredMatches.sort((a, b) => b.score - a.score);
    return scoredMatches;
}

/**
 * Score how well a shortcut matches the given arguments.
 * Higher score = better match.
 */
export function scoreShortcutMatch(shortcut, args) {
    const actionType = shortcut.action_type || 'run_program';
    const argCount = args.length;

    if (actionType === 'open_url') {
        const url = shortcut.url || '';
        const placeholderCount = (url.match(/\{\d+\}/g) || []).length;
        if (placeholderCount > 0) {
            if (argCount === placeholderCount) return 100;
            if (argCount > placeholderCount) return 80;
            return 60;
        }
        if (url.includes('{*}')) {
            return argCount > 0 ? 90 : 50;
        }
        return argCount === 0 ? 100 : 50;
    }

    // For run_program and others
    const argTemplate = shortcut.arguments || '';
    if (!argTemplate) {
        return argCount === 0 ? 100 : 50;
    }
    const placeholderCount = (argTemplate.match(/\{\d+\}/g) || []).length;
    if (placeholderCount > 0) {
        if (argCount === placeholderCount) return 100;
        if (argCount > placeholderCount) return 80;
        return 60;
    }
    if (argTemplate.includes('{*}')) {
        return argCount > 0 ? 90 : 50;
    }
    return argCount === 0 ? 100 : 50;
}

/**
 * Build a command object from a shortcut and arguments.
 * @param {Object} shortcut - Shortcut config object
 * @param {Array} args - Parsed arguments
 * @param {string} [selectionText] - Currently selected text from previous window
 * @returns {Object} Command object with type and relevant fields
 */
export function buildShortcutCommand(shortcut, args, selectionText = '') {
    const validation = validateShortcutArgs(shortcut, args);
    if (!validation.valid) {
        return { type: 'error', message: validation.message };
    }

    const actionType = shortcut.action_type || 'run_program';

    const substitute = (template, encode = false) => {
        if (!template) return '';
        let result = template;
        result = result.replace(/\{selection\}/g, encode ? encodeURIComponent(selectionText) : selectionText);
        if (result.includes('{*}')) {
            const all = args.join(' ');
            result = result.replace('{*}', encode ? encodeURIComponent(all) : all);
        } else {
            result = result.replace(/\{(\d+)\?\}/g, (_, idx) => {
                const i = parseInt(idx);
                const val = i < args.length ? args[i] : '';
                return encode ? encodeURIComponent(val) : val;
            });
            for (let i = 0; i < args.length; i++) {
                const val = encode ? encodeURIComponent(args[i]) : args[i];
                result = result.replace(new RegExp(`\\{${i}\\}`, 'g'), val);
            }
        }
        return result;
    };

    if (actionType === 'open_url') {
        return { type: 'open_url', url: substitute(shortcut.url || '', true) };
    }

    if (actionType === 'prompt') {
        return { type: 'prompt', message: substitute(shortcut.prompt || '{*}') };
    }

    if (actionType === 'script') {
        try {
            const fn = new Function('...args', shortcut.script || 'return args.join(" ")');
            const result = fn(...args);
            if (result === null || result === undefined) {
                return { type: 'noop' };
            }
            const scriptAction = shortcut.script_action || 'text';

            if (scriptAction === 'run_program') {
                if (!Array.isArray(result)) {
                    return { type: 'error', message: 'Script must return an array [cmd, workDir, ...args] for Run as Command' };
                }
                return {
                    type: 'run_program',
                    path: result[0] || '',
                    workDir: result[1] || null,
                    args: result.slice(2).map(String)
                };
            }

            if (typeof result !== 'string') {
                return { type: 'error', message: 'Script must return a string, got ' + typeof result };
            }
            if (scriptAction === 'open_url') return { type: 'open_url', url: result };
            if (scriptAction === 'prompt') return { type: 'prompt', message: result };
            return { type: 'text', message: result };
        } catch (e) {
            return { type: 'error', message: `Script ${e.constructor?.name || 'Error'}: ${e.message}` };
        }
    }

    // run_program (default)
    if (!shortcut.arguments) {
        return { type: 'run_program', path: shortcut.path, args: [], workDir: shortcut.working_directory };
    }
    const processedArgs = substitute(shortcut.arguments).split(/\s+/).filter(a => a && !a.match(/^\{\d+\}$/));
    return { type: 'run_program', path: shortcut.path, args: processedArgs, workDir: shortcut.working_directory };
}

/**
 * Validate that all required parameters are provided.
 */
export function validateShortcutArgs(shortcut, args) {
    const templates = [
        shortcut.url, shortcut.prompt, shortcut.arguments, shortcut.script
    ].filter(Boolean).join(' ');

    if (templates.includes('{*}')) return { valid: true };

    const requiredParams = new Set();
    const paramRegex = /\{(\d+)\}/g;
    let match;
    while ((match = paramRegex.exec(templates)) !== null) {
        const fullMatch = templates.substring(match.index, match.index + match[0].length + 1);
        if (!fullMatch.endsWith('?}')) {
            requiredParams.add(parseInt(match[1]));
        }
    }

    if (requiredParams.size === 0) return { valid: true };

    const maxRequired = Math.max(...requiredParams) + 1;
    if (args.length >= maxRequired) return { valid: true };

    const missing = maxRequired - args.length;
    return {
        valid: false,
        message: `This command requires ${maxRequired} parameter${maxRequired > 1 ? 's' : ''} (${missing} missing). Usage: ${shortcut.shortcut} <${Array.from(requiredParams).map(i => 'arg' + i).join('> <')}>`
    };
}

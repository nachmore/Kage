/**
 * Shortcut matching and command building — pure logic, no UI dependencies.
 * Reusable across floating and chat windows.
 *
 * Usage:
 *   import { matchShortcut, buildShortcutCommand } from './shortcuts.js';
 */

// ---------------------------------------------------------------------------
// Platform helpers — cross-platform key handling for shortcuts UX.
//
// Every window in the app is an ES module, so these are imported
// directly. Earlier revisions had a parallel
// `shared/platform-global.js` classic-script mirror exposing
// `window.kagePlatform` for non-module windows; that's gone now.
// ---------------------------------------------------------------------------

/**
 * True if the browser is running on macOS. WebView-on-macOS reports
 * "MacIntel" for both Apple Silicon and Intel; `navigator.userAgentData`
 * isn't widely available in all WebViews so we stick with the legacy API.
 */
let _isMacCached = null;
export function isMac() {
    if (_isMacCached === null) {
        _isMacCached =
            typeof navigator !== 'undefined' &&
            typeof navigator.platform === 'string' &&
            navigator.platform.startsWith('Mac');
    }
    return _isMacCached;
}

/**
 * True if the browser is running on Windows. Mirrors the Rust-side
 * `cfg(target_os = "windows")` gating so the UI can hide OS-specific
 * panes and features.
 */
let _isWindowsCached = null;
export function isWindows() {
    if (_isWindowsCached === null) {
        const plat =
            typeof navigator !== 'undefined' && typeof navigator.platform === 'string'
                ? navigator.platform
                : '';
        _isWindowsCached = plat.startsWith('Win');
    }
    return _isWindowsCached;
}

/** True if neither macOS nor Windows — i.e. Linux (and any other Unix fallthrough). */
let _isLinuxCached = null;
export function isLinux() {
    if (_isLinuxCached === null) {
        _isLinuxCached = !isMac() && !isWindows();
    }
    return _isLinuxCached;
}

/**
 * Uniform "command modifier" check for keyboard events.
 *
 * Mac: both Ctrl and ⌘ work (⌘ is idiomatic; labels rendered via
 * platformKeyLabel show ⌘). Windows/Linux: Ctrl only — Win/Super+key
 * combos are typically captured by the OS/WM and shouldn't hijack user
 * bindings that happen to leak through.
 *
 * @param {KeyboardEvent} e
 * @returns {boolean}
 */
export function cmdOrCtrlPressed(e) {
    return isMac() ? e.ctrlKey || e.metaKey : e.ctrlKey;
}

/**
 * Render a shortcut label with platform-appropriate modifier glyphs.
 *
 * On macOS "Ctrl+N" becomes "⌘N", "Ctrl+Shift+C" becomes "⌘⇧C", etc.
 * On non-mac the label is returned unchanged. Input uses "Ctrl" as the
 * platform-neutral token because that's what the rest of the codebase
 * stores.
 *
 * Unknown tokens pass through verbatim — we don't silently drop a "Win+"
 * or "Option+" that a user may have typed.
 *
 * @param {string} label
 * @returns {string}
 */
export function platformKeyLabel(label) {
    if (!isMac()) return label;
    return label
        .split('+')
        .map((part) => {
            switch (part.trim()) {
                case 'Ctrl':
                case 'Cmd':
                case 'Super':
                case 'Meta':
                    return '⌘';
                case 'Shift':
                    return '⇧';
                case 'Alt':
                case 'Option':
                    return '⌥';
                case 'Enter':
                case 'Return':
                    return '⏎';
                case 'Backspace':
                    return '⌫';
                case 'Escape':
                case 'Esc':
                    return '⎋';
                case 'Tab':
                    return '⇥';
                default:
                    return part;
            }
        })
        .join('');
}

// ---------------------------------------------------------------------------
// Placeholder parser & substitution.
//
// Templates support four kinds of placeholders:
//
//   {0}, {1}, ...  — numbered, value comes from args[index]
//   {N?}           — numbered + optional (won't trip validation if missing)
//   {name}, {n?}   — named, value comes from paramsByName[name] or, if
//                    absent, by consuming positional args left-to-right
//                    in *order of first appearance*. Repeating the same
//                    name re-uses the resolved value.
//   {*}            — wildcard, captures all args remaining after named
//                    placeholders have been consumed.
//   {selection}    — pulled from `selectionText` (separate source).
//
// Mixing numbered and named is unusual but legal — numbered always
// reads from args[index] (original list), named consumes a parallel
// queue from args[0]. Don't recommend mixing in the editor copy, but
// don't reject it either.
//
// `extractPlaceholders(template)` is the canonical scanner that the
// editor (to render placeholder chips), validation, scoring, and
// substitution all share.
// ---------------------------------------------------------------------------

const PLACEHOLDER_REGEX = /\{(\*|selection|[A-Za-z][A-Za-z0-9_-]*|\d+)(\?)?\}/g;

/**
 * Extract every placeholder from `template`, in order of appearance.
 * Returns an array of `{ raw, kind, name?, index?, optional }`.
 *   - `raw` is the literal token (`'{lang}'`, `'{0}'`, …) — used as the
 *     replacement key so the substitution loop can use plain string
 *     `split/join` and avoid re-escaping the regex.
 *   - `kind` is one of `numbered | named | wildcard | selection`.
 *
 * Pure + cheap; called from validation, scoring, and substitution.
 */
export function extractPlaceholders(template) {
    if (!template) return [];
    const out = [];
    let m;
    PLACEHOLDER_REGEX.lastIndex = 0;
    while ((m = PLACEHOLDER_REGEX.exec(template)) !== null) {
        const token = m[1];
        const optional = m[2] === '?';
        if (token === '*') {
            out.push({ raw: m[0], kind: 'wildcard', optional: false });
        } else if (token === 'selection') {
            out.push({ raw: m[0], kind: 'selection', optional: false });
        } else if (/^\d+$/.test(token)) {
            out.push({
                raw: m[0],
                kind: 'numbered',
                index: parseInt(token, 10),
                optional,
            });
        } else {
            out.push({ raw: m[0], kind: 'named', name: token, optional });
        }
    }
    return out;
}

/**
 * Summarise the named placeholders in a shortcut, deduped. Used by the
 * settings editor's "Detected placeholders" chip row so the user can
 * see exactly what their template will ask for.
 *
 * Returns `[{ name, optional }]` in order of first appearance.
 */
export function summarizeNamedPlaceholders(template) {
    const placeholders = extractPlaceholders(template);
    const seen = new Map(); // name → { name, optional }
    for (const p of placeholders) {
        if (p.kind !== 'named') continue;
        if (!seen.has(p.name)) {
            seen.set(p.name, { name: p.name, optional: p.optional });
        } else if (!p.optional) {
            // Promote to required if any occurrence is required — being
            // conservative is the right call for the editor copy.
            seen.get(p.name).optional = false;
        }
    }
    return [...seen.values()];
}

/**
 * Compute which named placeholders across all shortcut templates are
 * unfilled, given args + paramsByName. Used by validation + the form
 * trigger in the launcher.
 *
 * Returns:
 *   - `filled: { [name]: value }` — final resolution of named params,
 *     including those consumed from `args`.
 *   - `unfilled: [{ name, optional }]` — named params with no value;
 *     the launcher pops a form for these (required-first ordering).
 *   - `argsConsumedByNamed` — count of `args` consumed by named
 *     placeholders, so the wildcard substitution can use the rest.
 */
export function resolveNamedPlaceholders(templates, args, paramsByName = {}) {
    const namedOrder = []; // unique names in order of first appearance
    const optionalByName = new Map();
    for (const tmpl of templates) {
        for (const p of extractPlaceholders(tmpl || '')) {
            if (p.kind !== 'named') continue;
            if (!namedOrder.includes(p.name)) {
                namedOrder.push(p.name);
                optionalByName.set(p.name, p.optional);
            } else if (!p.optional) {
                optionalByName.set(p.name, false);
            }
        }
    }
    const filled = {};
    const unfilled = [];
    let argIdx = 0;
    for (const name of namedOrder) {
        if (paramsByName[name] !== undefined && paramsByName[name] !== null) {
            filled[name] = paramsByName[name];
            continue;
        }
        if (argIdx < args.length) {
            filled[name] = args[argIdx];
            argIdx += 1;
            continue;
        }
        unfilled.push({ name, optional: optionalByName.get(name) ?? false });
    }
    return { filled, unfilled, argsConsumedByNamed: argIdx };
}

// ---------------------------------------------------------------------------
// Shortcut matching (pre-existing logic)
// ---------------------------------------------------------------------------

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

    const matches = shortcuts.filter((s) => s.shortcut.toLowerCase() === trigger);
    if (matches.length === 0) return null;

    const scoredMatches = matches.map((shortcut) => {
        const score = scoreShortcutMatch(shortcut, args);
        return { shortcut, args, score };
    });

    scoredMatches.sort((a, b) => b.score - a.score);
    return scoredMatches;
}

/**
 * Score how well a shortcut matches the given arguments.
 * Higher score = better match. Named placeholders count toward the
 * required-arg total (they're filled left-to-right from `args` when
 * not provided explicitly), so a `{lang}` template scores the same as
 * a `{0}` template for arg counting purposes.
 */
export function scoreShortcutMatch(shortcut, args) {
    const actionType = shortcut.action_type || 'run_program';
    const argCount = args.length;

    const templateForType = (() => {
        if (actionType === 'open_url') return shortcut.url || '';
        if (actionType === 'prompt') return shortcut.prompt || '';
        if (actionType === 'script') return shortcut.script || '';
        return shortcut.arguments || '';
    })();

    const placeholders = extractPlaceholders(templateForType);
    const numbered = placeholders.filter((p) => p.kind === 'numbered' && !p.optional);
    const named = new Set(
        placeholders.filter((p) => p.kind === 'named' && !p.optional).map((p) => p.name)
    );
    const hasWildcard = placeholders.some((p) => p.kind === 'wildcard');
    const requiredCount =
        (numbered.length ? Math.max(...numbered.map((p) => p.index)) + 1 : 0) + named.size;

    if (requiredCount === 0 && !hasWildcard) {
        // Bare template with no placeholders — exact match if arg-less,
        // worse otherwise. This is the legacy "open VSCode" / "no
        // template" path: typing extra args still scores 50 so we don't
        // accidentally suppress the match entirely.
        return argCount === 0 ? 100 : 50;
    }

    if (hasWildcard) {
        // Wildcard absorbs any number of args; we still want the strong
        // 90 score when SOMETHING was passed (intent likelier matches).
        if (requiredCount > 0 && argCount < requiredCount) return 60;
        return argCount > 0 ? 90 : 50;
    }

    if (argCount === requiredCount) return 100;
    if (argCount > requiredCount) return 80;
    return 60;
}

/**
 * Build a command object from a shortcut and arguments.
 * @param {Object} shortcut - Shortcut config object
 * @param {Array} args - Parsed arguments
 * @param {string} [selectionText] - Currently selected text from previous window
 * @param {Object} [paramsByName] - Already-filled named placeholders, keyed
 *   by placeholder name. The launcher passes this back after collecting the
 *   form. Empty by default — initial calls let positional args fill named
 *   placeholders.
 * @returns {Object} Command object with type and relevant fields. If a
 *   `prompt`-type shortcut still has unfilled named placeholders after
 *   resolution, returns `{ type: 'prompt_form', shortcut, args, prefilled,
 *   missing }` instead so the caller can pop a form.
 */
export function buildShortcutCommand(shortcut, args, selectionText = '', paramsByName = {}) {
    const actionType = shortcut.action_type || 'run_program';

    // Resolve named placeholders ahead of validation so we can tell the
    // difference between "arg missing" and "form needed."
    const allTemplates = [shortcut.url, shortcut.prompt, shortcut.arguments, shortcut.script];
    const namedResolution = resolveNamedPlaceholders(allTemplates, args, paramsByName);
    // Remaining positional args after named consumption — used when the
    // template *also* uses {*} or {0..n} numbered placeholders.
    const remainingArgs = args.slice(namedResolution.argsConsumedByNamed);

    const validation = validateShortcutArgs(shortcut, args, paramsByName);
    if (!validation.valid) {
        // Special-case: prompt-type with only-named-placeholders missing.
        // Surface a `prompt_form` so the launcher can collect them
        // interactively rather than erroring out.
        if (actionType === 'prompt' && namedResolution.unfilled.length > 0) {
            const onlyNamedMissing = !validation.missingNumbered;
            if (onlyNamedMissing) {
                return {
                    type: 'prompt_form',
                    shortcut,
                    args,
                    prefilled: namedResolution.filled,
                    missing: namedResolution.unfilled,
                };
            }
        }
        return { type: 'error', message: validation.message };
    }

    const substitute = (template, encode = false) => {
        if (!template) return '';
        let result = template;
        const enc = (v) => (encode ? encodeURIComponent(v) : v);

        result = result.replace(/\{selection\}/g, enc(selectionText));

        // Named placeholders first — replace by literal token so we don't
        // need to escape anything. Both required and optional forms.
        for (const [name, value] of Object.entries(namedResolution.filled)) {
            result = result.split(`{${name}}`).join(enc(value));
            result = result.split(`{${name}?}`).join(enc(value));
        }
        // Strip any unfilled optional named placeholders that survived.
        result = result.replace(/\{[A-Za-z][A-Za-z0-9_-]*\?\}/g, '');

        if (result.includes('{*}')) {
            // {*} captures whatever's left after named placeholders ate
            // their portion. Wildcard captures the original input minus
            // named-consumed prefix — matches user expectation: "first
            // arg goes to {lang}, then the rest is the body."
            const all = remainingArgs.join(' ');
            result = result.split('{*}').join(enc(all));
        } else {
            result = result.replace(/\{(\d+)\?\}/g, (_, idx) => {
                const i = parseInt(idx, 10);
                return enc(i < args.length ? args[i] : '');
            });
            for (let i = 0; i < args.length; i++) {
                result = result.split(`{${i}}`).join(enc(args[i]));
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
                    return {
                        type: 'error',
                        message:
                            'Script must return an array [cmd, workDir, ...args] for Run as Command',
                    };
                }
                return {
                    type: 'run_program',
                    path: result[0] || '',
                    workDir: result[1] || null,
                    args: result.slice(2).map(String),
                };
            }

            if (typeof result !== 'string') {
                return {
                    type: 'error',
                    message: 'Script must return a string, got ' + typeof result,
                };
            }
            if (scriptAction === 'open_url') return { type: 'open_url', url: result };
            if (scriptAction === 'prompt') return { type: 'prompt', message: result };
            return { type: 'text', message: result };
        } catch (e) {
            return {
                type: 'error',
                message: `Script ${e.constructor?.name || 'Error'}: ${e.message}`,
            };
        }
    }

    // run_program (default)
    if (!shortcut.arguments) {
        return {
            type: 'run_program',
            path: shortcut.path,
            args: [],
            workDir: shortcut.working_directory,
        };
    }
    const processedArgs = substitute(shortcut.arguments)
        .split(/\s+/)
        .filter((a) => a && !a.match(/^\{\d+\}$/));
    return {
        type: 'run_program',
        path: shortcut.path,
        args: processedArgs,
        workDir: shortcut.working_directory,
    };
}

/**
 * Validate that all required parameters are provided.
 *
 * Numbered placeholders ({0}, {1}, …) consume args by index. Named
 * placeholders ({lang}, {level}, …) consume args left-to-right after
 * any explicit values in `paramsByName`. Optional forms ({0?},
 * {name?}) never trip validation.
 *
 * Returns `{ valid, message?, missingNumbered? }`. `missingNumbered` is
 * true when at least one *numbered* required placeholder couldn't be
 * filled — the caller (buildShortcutCommand) uses that to decide
 * whether to fall back to the inline form (named-only) or surface a
 * usage error (numbered or mixed).
 */
export function validateShortcutArgs(shortcut, args, paramsByName = {}) {
    const templates = [shortcut.url, shortcut.prompt, shortcut.arguments, shortcut.script];
    const joined = templates.filter(Boolean).join(' ');

    // Wildcard absorbs anything except *named* requirements — we still
    // need to make sure named slots get values one way or another.
    const placeholders = templates.flatMap((t) => extractPlaceholders(t || ''));
    const requiredNamed = new Set();
    for (const p of placeholders) {
        if (p.kind === 'named' && !p.optional) requiredNamed.add(p.name);
    }

    const requiredNumbered = new Set();
    for (const p of placeholders) {
        if (p.kind === 'numbered' && !p.optional) requiredNumbered.add(p.index);
    }

    const hasWildcard = joined.includes('{*}');

    // Resolve named first — they consume args left-to-right.
    const namedResolution = resolveNamedPlaceholders(templates, args, paramsByName);
    const namedMissing = namedResolution.unfilled.filter((p) => requiredNamed.has(p.name));

    // Numbered placeholders read directly from args[] (original list,
    // not the post-named slice — `{0}` always means "first arg").
    const numberedMissing = [];
    for (const idx of requiredNumbered) {
        if (idx >= args.length) numberedMissing.push(idx);
    }

    if (namedMissing.length === 0 && numberedMissing.length === 0) {
        return { valid: true };
    }
    if (hasWildcard && requiredNumbered.size === 0 && requiredNamed.size === 0) {
        return { valid: true };
    }

    // Build a usage hint that's accurate when only some placeholders
    // are missing. Named names are friendlier than positional indices.
    const parts = [];
    for (const idx of [...requiredNumbered].sort((a, b) => a - b)) parts.push(`arg${idx}`);
    for (const p of namedResolution.unfilled.filter((p) => requiredNamed.has(p.name))) {
        parts.push(p.name);
    }
    const totalMissing = namedMissing.length + numberedMissing.length;

    return {
        valid: false,
        missingNumbered: numberedMissing.length > 0,
        message: `This command needs ${totalMissing} more parameter${totalMissing > 1 ? 's' : ''}. Usage: ${shortcut.shortcut} <${parts.join('> <')}>`,
    };
}

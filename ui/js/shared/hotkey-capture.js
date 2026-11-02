/**
 * Pure helpers for recording a hotkey from DOM keyboard events.
 *
 * The hotkey picker uses these on macOS, where recording inside the
 * focused Settings / welcome webview needs no TCC permission. The
 * alternative — a CGEventTap — needs Accessibility, and TCC pins that
 * grant to the exact binary hash, so every update of an ad-hoc-signed
 * build silently revoked it while System Settings still showed Kage as
 * approved ("asks me to approve Kage but it's already approved").
 *
 * Key names match what the Rust side stores in config and what
 * global-hotkey's parser accepts (see src/os/macos/keycodes.rs), and
 * modifiers come out in the same Ctrl, Alt, Shift, Super order the native
 * capture paths use.
 */

/** `KeyboardEvent.code` → Kage key name, for codes that aren't a simple pattern. */
const CODE_TO_KEY = Object.freeze({
    Space: 'Space',
    Enter: 'Enter',
    Tab: 'Tab',
    Backspace: 'Backspace',
    Delete: 'Delete',
    Insert: 'Insert',
    Home: 'Home',
    End: 'End',
    PageUp: 'PageUp',
    PageDown: 'PageDown',
    ArrowLeft: 'Left',
    ArrowRight: 'Right',
    ArrowUp: 'Up',
    ArrowDown: 'Down',
    Minus: '-',
    Equal: '=',
    BracketLeft: '[',
    BracketRight: ']',
    Backslash: '\\',
    Semicolon: ';',
    Quote: "'",
    Comma: ',',
    Period: '.',
    Slash: '/',
    Backquote: '`',
    NumpadDecimal: 'NumDecimal',
    NumpadMultiply: 'NumMultiply',
    NumpadAdd: 'NumAdd',
    NumpadSubtract: 'NumSubtract',
    NumpadDivide: 'NumDivide',
    NumpadEnter: 'NumEnter',
    NumpadEqual: 'NumEqual',
});

const MODIFIER_CODES = new Set([
    'ShiftLeft',
    'ShiftRight',
    'ControlLeft',
    'ControlRight',
    'AltLeft',
    'AltRight',
    'MetaLeft',
    'MetaRight',
    'OSLeft',
    'OSRight',
    'CapsLock',
    'Fn',
    'FnLock',
]);

const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'OS', 'CapsLock', 'Fn', 'FnLock']);

/**
 * Map a `KeyboardEvent.code` to Kage's key name, or `null` if Kage can't
 * bind it. Uses the physical `code` (not `key`) so Option+A gives "A"
 * rather than "å", and the result is layout-independent like the native
 * capture paths.
 */
export function codeToKeyName(code) {
    if (typeof code !== 'string' || !code) return null;
    let m = /^Key([A-Z])$/.exec(code);
    if (m) return m[1];
    m = /^Digit([0-9])$/.exec(code);
    if (m) return m[1];
    m = /^Numpad([0-9])$/.exec(code);
    if (m) return 'Num' + m[1];
    if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
    return Object.prototype.hasOwnProperty.call(CODE_TO_KEY, code) ? CODE_TO_KEY[code] : null;
}

/** Modifier names held during `e`, in Ctrl, Alt, Shift, Super order. */
export function modifiersFromEvent(e) {
    const mods = [];
    if (e.ctrlKey) mods.push('Ctrl');
    if (e.altKey) mods.push('Alt');
    if (e.shiftKey) mods.push('Shift');
    if (e.metaKey) mods.push('Super');
    return mods;
}

/**
 * Classify a keydown during recording:
 *   { kind: 'ignore' }                    modifier-only press, auto-repeat or IME composition — keep waiting
 *   { kind: 'cancel' }                    bare Escape
 *   { kind: 'unsupported', code }         a key Kage can't bind — keep waiting
 *   { kind: 'combo', modifiers, key }     done
 */
export function classifyKeyEvent(e) {
    if (!e || e.repeat || e.isComposing || e.keyCode === 229) return { kind: 'ignore' };
    if (MODIFIER_CODES.has(e.code) || MODIFIER_KEYS.has(e.key)) return { kind: 'ignore' };
    const modifiers = modifiersFromEvent(e);
    if (e.code === 'Escape') {
        return modifiers.length === 0 ? { kind: 'cancel' } : { kind: 'combo', modifiers, key: 'Escape' };
    }
    const key = codeToKeyName(e.code);
    if (!key) return { kind: 'unsupported', code: e.code || e.key || '' };
    return { kind: 'combo', modifiers, key };
}

const MAC_SYMBOLS = Object.freeze({ Ctrl: '⌃', Alt: '⌥', Shift: '⇧', Super: '⌘' });

/** Human label for a combo, e.g. "⌘Space" on macOS or "Ctrl+Space" elsewhere. */
export function formatCombo(hk, { mac = false } = {}) {
    const mods = hk?.modifiers || [];
    const key = hk?.key || '';
    if (mac) return mods.map((m) => MAC_SYMBOLS[m] || m).join('') + key;
    return [...mods, key].filter(Boolean).join('+');
}

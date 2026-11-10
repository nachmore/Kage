const URLS_CAP_ALLOWED_SCHEMES = Object.freeze([
    'http',
    'https',
    'mailto',
    'tel',
    'sms',
    'facetime',
    'facetime-audio',
    'imessage',
    'x-apple.systempreferences',
    'ms-settings',
    'prefs',
]);

function extractScheme(url) {
    if (typeof url !== 'string') return null;
    const trimmed = url.replace(/^\s+/, '');
    if (/^www\./i.test(trimmed)) return 'https';
    const match = trimmed.match(/^([a-zA-Z][a-zA-Z0-9+.-]*):/);
    return match ? match[1].toLowerCase() : null;
}

/** Every file text format id Rust knows (`Format` in kage-core). */
export const FILE_TEXT_FORMATS = Object.freeze([
    'text',
    'markdown',
    'pdf',
    'docx',
    'html',
    'pptx',
    'rtf',
]);
/** The largest `max_chars` an extension may ask `pick_file_text` for. */
export const MAX_FILE_TEXT_CHARS = 1_000_000;

function isPlainObject(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
    const proto = Object.getPrototypeOf(value);
    return proto === Object.prototype || proto === null;
}

function describe(value) {
    return typeof value === 'string' ? `'${value.slice(0, 32)}'` : typeof value;
}

/**
 * `pick_file_text`'s arguments (docs/design/EXTENSION_FILE_TEXT.md §3.9):
 * `args` is absent or an object with only `formats` (1–7 known ids, no
 * duplicates) and `max_chars` (a whole number from 1 to 1,000,000). Other
 * top-level keys are left alone: the host overwrites the identity and
 * Tauri ignores the rest. Rust checks again.
 */
function validatePickFileTextArgs(invokeArgs) {
    const args = invokeArgs?.args;
    if (args === undefined || args === null) return null;
    if (!isPlainObject(args)) return "pick_file_text: 'args' must be an object.";
    for (const key of Object.keys(args)) {
        if (key !== 'formats' && key !== 'max_chars') {
            return `pick_file_text: unknown argument '${key.slice(0, 32)}'. Allowed: formats, max_chars.`;
        }
    }
    const known = FILE_TEXT_FORMATS.join(', ');
    const { formats, max_chars: maxChars } = args;
    if (formats !== undefined && formats !== null) {
        if (
            !Array.isArray(formats) ||
            formats.length < 1 ||
            formats.length > FILE_TEXT_FORMATS.length
        ) {
            return `pick_file_text: 'formats' must list 1 to ${FILE_TEXT_FORMATS.length} of: ${known}.`;
        }
        const seen = new Set();
        for (const f of formats) {
            if (!FILE_TEXT_FORMATS.includes(f)) {
                return `pick_file_text: unknown format ${describe(f)}. Known: ${known}.`;
            }
            if (seen.has(f)) return `pick_file_text: format '${f}' is listed twice.`;
            seen.add(f);
        }
    }
    if (maxChars !== undefined && maxChars !== null) {
        if (!Number.isInteger(maxChars) || maxChars < 1 || maxChars > MAX_FILE_TEXT_CHARS) {
            return `pick_file_text: 'max_chars' must be a whole number from 1 to ${MAX_FILE_TEXT_CHARS}.`;
        }
    }
    return null;
}

function validateOpenUrlArgs(args) {
    const url = args?.url;
    if (typeof url !== 'string' || !url.trim()) {
        return "open_url called with no 'url' argument.";
    }
    const scheme = extractScheme(url);
    if (!scheme) {
        return `open_url rejected: '${url}' has no scheme. Use http(s), mailto, tel, or an OS-settings deep link.`;
    }
    if (!URLS_CAP_ALLOWED_SCHEMES.includes(scheme)) {
        return `open_url rejected: scheme '${scheme}:' is not allowed for the 'urls' capability. Custom app URI schemes need the 'launch' capability.`;
    }
    return null;
}

/** Return an extension-safe argument validation error, or null when valid. */
export function validateExtensionInvokeArgs(command, args) {
    if (command === 'open_url') return validateOpenUrlArgs(args);
    if (command === 'pick_file_text') return validatePickFileTextArgs(args);
    return null;
}

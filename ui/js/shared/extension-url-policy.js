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

/** Return an extension-safe argument validation error, or null when valid. */
export function validateExtensionInvokeArgs(command, args) {
    if (command !== 'open_url') return null;

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

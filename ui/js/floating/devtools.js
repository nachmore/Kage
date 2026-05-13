/**
 * Developer tools for the floating window.
 * UUID generation, base64 encode/decode, hash calculation,
 * epoch/date conversion, JSON formatting.
 */

import { escapeHtml } from '../shared/tool-utils.js';

/**
 * Try to match a dev tool command. Returns { type, label, icon, value } or null.
 * @param {string} input - trimmed user input
 * @param {object} config - dev_tools config { enabled, uuid, base64, hash, epoch, json_format }
 */
export function matchDevTool(input, config) {
    if (!config?.enabled) return null;
    const lower = input.toLowerCase().trim();

    // UUID generator
    if (config.uuid && (lower === 'uuid' || lower === 'guid' || lower === 'uuid4')) {
        const uuid = crypto.randomUUID();
        return {
            type: 'devtool',
            label: 'UUID v4',
            icon: '🔑',
            value: uuid,
            description: 'Press Enter to copy',
        };
    }

    // Base64 encode: "base64 <text>" or "b64 <text>"
    if (config.base64 && /^(base64|b64)\s+.+/i.test(lower)) {
        const text = input.replace(/^(base64|b64)\s+/i, '');
        try {
            const encoded = btoa(unescape(encodeURIComponent(text)));
            return {
                type: 'devtool',
                label: 'Base64 Encode',
                icon: '📦',
                value: encoded,
                description: `"${text}" → Enter to copy`,
            };
        } catch {
            return null;
        }
    }

    // Base64 decode: "base64d <text>" or "b64d <text>" or "decode <text>"
    if (config.base64 && /^(base64d|b64d|decode)\s+.+/i.test(lower)) {
        const text = input.replace(/^(base64d|b64d|decode)\s+/i, '').trim();
        try {
            const decoded = decodeURIComponent(escape(atob(text)));
            return {
                type: 'devtool',
                label: 'Base64 Decode',
                icon: '📭',
                value: decoded,
                description: 'Enter to copy',
            };
        } catch {
            return null;
        }
    }

    // Hash: "md5 <text>", "sha1 <text>", "sha256 <text>", "sha512 <text>"
    if (config.hash && /^(md5|sha1|sha256|sha512)\s+.+/i.test(lower)) {
        const match = input.match(/^(md5|sha1|sha256|sha512)\s+(.+)/i);
        if (match) {
            const algo = match[1].toLowerCase();
            const text = match[2];
            // Return a pending result — hash is async
            return {
                type: 'devtool_async',
                algo,
                text,
                label: `${algo.toUpperCase()} Hash`,
                icon: '#️⃣',
                description: 'Computing...',
            };
        }
    }

    // Epoch/date: Unix timestamp (10 or 13 digits)
    if (config.epoch && /^\d{10,13}$/.test(lower)) {
        const ts = lower.length === 13 ? parseInt(lower, 10) : parseInt(lower, 10) * 1000;
        const date = new Date(ts);
        if (!Number.isNaN(date.getTime())) {
            const local = date.toLocaleString();
            const iso = date.toISOString();
            return {
                type: 'devtool',
                label: 'Epoch → Date',
                icon: '🕐',
                value: `${local}\n${iso}`,
                description: local,
            };
        }
    }

    // "now" or "epoch" → current timestamp
    if (config.epoch && (lower === 'now' || lower === 'epoch' || lower === 'timestamp')) {
        const now = Date.now();
        const secs = Math.floor(now / 1000);
        const local = new Date(now).toLocaleString();
        return {
            type: 'devtool',
            label: 'Current Epoch',
            icon: '🕐',
            value: secs.toString(),
            description: `${secs} (${local})`,
        };
    }

    // JSON format: detect pasted minified JSON
    if (config.json_format && (lower.startsWith('{') || lower.startsWith('['))) {
        try {
            const parsed = JSON.parse(input);
            const formatted = JSON.stringify(parsed, null, 2);
            // Only show if it's actually minified (no newlines in original)
            if (!input.includes('\n') && formatted.includes('\n')) {
                return {
                    type: 'devtool',
                    label: 'Format JSON',
                    icon: '📐',
                    value: formatted,
                    description: `${Object.keys(parsed).length || '?'} keys · Enter to copy`,
                };
            }
        } catch {
            /* not valid JSON */
        }
    }

    return null;
}

/**
 * Compute a hash asynchronously using Web Crypto API.
 * @returns {Promise<string>} hex hash string
 */
export async function computeHash(algo, text) {
    const encoder = new TextEncoder();
    const data = encoder.encode(text);

    // Map friendly names to Web Crypto algorithm names
    const algoMap = { sha1: 'SHA-1', sha256: 'SHA-256', sha512: 'SHA-512' };
    const webCryptoAlgo = algoMap[algo];

    if (webCryptoAlgo) {
        const hashBuffer = await crypto.subtle.digest(webCryptoAlgo, data);
        const hashArray = Array.from(new Uint8Array(hashBuffer));
        return hashArray.map((b) => b.toString(16).padStart(2, '0')).join('');
    }

    // MD5 — simple implementation (not in Web Crypto)
    if (algo === 'md5') {
        return md5(text);
    }

    return null;
}

// --- Minimal MD5 implementation ---
function md5(string) {
    function md5cycle(x, k) {
        let a = x[0],
            b = x[1],
            c = x[2],
            d = x[3];
        a = ff(a, b, c, d, k[0], 7, -680876936);
        d = ff(d, a, b, c, k[1], 12, -389564586);
        c = ff(c, d, a, b, k[2], 17, 606105819);
        b = ff(b, c, d, a, k[3], 22, -1044525330);
        a = ff(a, b, c, d, k[4], 7, -176418897);
        d = ff(d, a, b, c, k[5], 12, 1200080426);
        c = ff(c, d, a, b, k[6], 17, -1473231341);
        b = ff(b, c, d, a, k[7], 22, -45705983);
        a = ff(a, b, c, d, k[8], 7, 1770035416);
        d = ff(d, a, b, c, k[9], 12, -1958414417);
        c = ff(c, d, a, b, k[10], 17, -42063);
        b = ff(b, c, d, a, k[11], 22, -1990404162);
        a = ff(a, b, c, d, k[12], 7, 1804603682);
        d = ff(d, a, b, c, k[13], 12, -40341101);
        c = ff(c, d, a, b, k[14], 17, -1502002290);
        b = ff(b, c, d, a, k[15], 22, 1236535329);
        a = gg(a, b, c, d, k[1], 5, -165796510);
        d = gg(d, a, b, c, k[6], 9, -1069501632);
        c = gg(c, d, a, b, k[11], 14, 643717713);
        b = gg(b, c, d, a, k[0], 20, -373897302);
        a = gg(a, b, c, d, k[5], 5, -701558691);
        d = gg(d, a, b, c, k[10], 9, 38016083);
        c = gg(c, d, a, b, k[15], 14, -660478335);
        b = gg(b, c, d, a, k[4], 20, -405537848);
        a = gg(a, b, c, d, k[9], 5, 568446438);
        d = gg(d, a, b, c, k[14], 9, -1019803690);
        c = gg(c, d, a, b, k[3], 14, -187363961);
        b = gg(b, c, d, a, k[8], 20, 1163531501);
        a = gg(a, b, c, d, k[13], 5, -1444681467);
        d = gg(d, a, b, c, k[2], 9, -51403784);
        c = gg(c, d, a, b, k[7], 14, 1735328473);
        b = gg(b, c, d, a, k[12], 20, -1926607734);
        a = hh(a, b, c, d, k[5], 4, -378558);
        d = hh(d, a, b, c, k[8], 11, -2022574463);
        c = hh(c, d, a, b, k[11], 16, 1839030562);
        b = hh(b, c, d, a, k[14], 23, -35309556);
        a = hh(a, b, c, d, k[1], 4, -1530992060);
        d = hh(d, a, b, c, k[4], 11, 1272893353);
        c = hh(c, d, a, b, k[7], 16, -155497632);
        b = hh(b, c, d, a, k[10], 23, -1094730640);
        a = hh(a, b, c, d, k[13], 4, 681279174);
        d = hh(d, a, b, c, k[0], 11, -358537222);
        c = hh(c, d, a, b, k[3], 16, -722521979);
        b = hh(b, c, d, a, k[6], 23, 76029189);
        a = hh(a, b, c, d, k[9], 4, -640364487);
        d = hh(d, a, b, c, k[12], 11, -421815835);
        c = hh(c, d, a, b, k[15], 16, 530742520);
        b = hh(b, c, d, a, k[2], 23, -995338651);
        a = ii(a, b, c, d, k[0], 6, -198630844);
        d = ii(d, a, b, c, k[7], 10, 1126891415);
        c = ii(c, d, a, b, k[14], 15, -1416354905);
        b = ii(b, c, d, a, k[5], 21, -57434055);
        a = ii(a, b, c, d, k[12], 6, 1700485571);
        d = ii(d, a, b, c, k[3], 10, -1894986606);
        c = ii(c, d, a, b, k[10], 15, -1051523);
        b = ii(b, c, d, a, k[1], 21, -2054922799);
        a = ii(a, b, c, d, k[8], 6, 1873313359);
        d = ii(d, a, b, c, k[15], 10, -30611744);
        c = ii(c, d, a, b, k[6], 15, -1560198380);
        b = ii(b, c, d, a, k[13], 21, 1309151649);
        a = ii(a, b, c, d, k[4], 6, -145523070);
        d = ii(d, a, b, c, k[11], 10, -1120210379);
        c = ii(c, d, a, b, k[2], 15, 718787259);
        b = ii(b, c, d, a, k[9], 21, -343485551);
        x[0] = add32(a, x[0]);
        x[1] = add32(b, x[1]);
        x[2] = add32(c, x[2]);
        x[3] = add32(d, x[3]);
    }
    function cmn(q, a, b, x, s, t) {
        a = add32(add32(a, q), add32(x, t));
        return add32((a << s) | (a >>> (32 - s)), b);
    }
    function ff(a, b, c, d, x, s, t) {
        return cmn((b & c) | (~b & d), a, b, x, s, t);
    }
    function gg(a, b, c, d, x, s, t) {
        return cmn((b & d) | (c & ~d), a, b, x, s, t);
    }
    function hh(a, b, c, d, x, s, t) {
        return cmn(b ^ c ^ d, a, b, x, s, t);
    }
    function ii(a, b, c, d, x, s, t) {
        return cmn(c ^ (b | ~d), a, b, x, s, t);
    }
    function md5blk(s) {
        const md5blks = [];
        for (let i = 0; i < 64; i += 4)
            md5blks[i >> 2] =
                s.charCodeAt(i) +
                (s.charCodeAt(i + 1) << 8) +
                (s.charCodeAt(i + 2) << 16) +
                (s.charCodeAt(i + 3) << 24);
        return md5blks;
    }
    function add32(a, b) {
        return (a + b) & 0xffffffff;
    }

    let n = string.length,
        state = [1732584193, -271733879, -1732584194, 271733878],
        i;
    for (i = 64; i <= n; i += 64) md5cycle(state, md5blk(string.substring(i - 64, i)));
    string = string.substring(i - 64);
    const tail = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    for (i = 0; i < string.length; i++) tail[i >> 2] |= string.charCodeAt(i) << ((i % 4) << 3);
    tail[i >> 2] |= 0x80 << ((i % 4) << 3);
    if (i > 55) {
        md5cycle(state, tail);
        for (i = 0; i < 16; i++) tail[i] = 0;
    }
    tail[14] = n * 8;
    md5cycle(state, tail);

    const hex = '0123456789abcdef';
    let s = '';
    for (i = 0; i < 4; i++)
        for (let j = 0; j < 4; j++)
            s +=
                hex.charAt((state[i] >> (j * 8 + 4)) & 0xf) +
                hex.charAt((state[i] >> (j * 8)) & 0xf);
    return s;
}

/**
 * Render a dev tool result as a suggestion item.
 */
export function renderDevToolSuggestion(result, container, currentMatches, resizeWindow) {
    container.innerHTML = '';
    container.scrollTop = 0;
    currentMatches.length = 0;
    currentMatches.push(result);

    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';
    item.innerHTML = `
        <div class="app-icon">${result.icon}</div>
        <div class="app-info">
            <div class="app-name" style="font-family:monospace;font-size:13px;user-select:text;">${escapeHtml(result.value)}</div>
            <div class="app-description">${result.label} · ${result.description}</div>
        </div>
    `;
    container.appendChild(item);
    container.classList.add('visible');
    resizeWindow();
    return 0;
}

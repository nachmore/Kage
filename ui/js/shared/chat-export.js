/**
 * Chat-session → Markdown serializer.
 *
 * Pure: turns the chat window's in-memory `messages` array (and a few
 * bits of session metadata) into a single Markdown string the user can
 * save anywhere. No DOM, no IPC — easy to unit-test and reuse in
 * future surfaces (export-all, "send to Slack" extensions, etc.).
 *
 * Output shape:
 *
 *   # <session title>
 *   _Exported on YYYY-MM-DD · model · session id_
 *
 *   ## You
 *   <user message>
 *
 *   ## Kage
 *   <assistant message — markdown is emitted as-is, since the agent
 *   already produces markdown>
 *
 * Empty messages are skipped. Roles other than 'user' / 'assistant'
 * (rare — we use 'system' for some surfaces) get a generic heading.
 */

/** Map a stored role name → the human-readable header used in the export. */
function roleHeading(role) {
    switch (role) {
        case 'user':
            return '## You';
        case 'assistant':
            return '## Kage';
        case 'system':
            return '## System';
        default:
            return `## ${String(role || 'message')}`;
    }
}

/**
 * Build a markdown document from a chat session.
 *
 * @param {Object} args
 * @param {Array<{role: string, content: string}>} args.messages
 * @param {string} [args.title] — session title; defaults to "Untitled chat"
 * @param {string} [args.model] — model name; included in the metadata line
 * @param {string} [args.sessionId] — short id; included in the metadata line
 * @param {string} [args.exportedAt] — ISO date for the metadata line; defaults to today (UTC)
 * @returns {string} the full markdown document
 */
export function buildChatMarkdown({ messages, title, model, sessionId, exportedAt } = {}) {
    const safeTitle = (title && String(title).trim()) || 'Untitled chat';
    const date = exportedAt || new Date().toISOString().slice(0, 10); // YYYY-MM-DD in UTC

    const metaParts = [`Exported on ${date}`];
    if (model) metaParts.push(String(model));
    if (sessionId) metaParts.push(`session ${String(sessionId).slice(0, 8)}`);

    const out = [];
    out.push(`# ${safeTitle}`);
    out.push('');
    out.push(`_${metaParts.join(' · ')}_`);
    out.push('');

    if (!Array.isArray(messages)) {
        return out.join('\n');
    }

    for (const msg of messages) {
        if (!msg || typeof msg !== 'object') continue;
        const content = typeof msg.content === 'string' ? msg.content.trim() : '';
        if (!content) continue;
        out.push(roleHeading(msg.role || ''));
        out.push('');
        out.push(content);
        out.push('');
    }

    // Trim trailing blank lines and finish with exactly one newline so
    // git / less treat the file as well-formed.
    while (out.length > 0 && out[out.length - 1] === '') out.pop();
    out.push('');
    return out.join('\n');
}

/** Sanitise a session title into something safe to use as a default filename. */
export function defaultExportFilename(title) {
    const base = (title && String(title).trim()) || 'kage-chat';
    // Keep it Windows-safe: drop the OS-reserved chars + control bytes.
    // Leave Unicode letters alone (ja/zh/ar titles deserve to round-trip).
    // Build the control-byte range from char codes rather than `\x00`
    // escapes so Biome's `noControlCharactersInRegex` doesn't fire —
    // semantically identical, just sourced via `String.fromCharCode`
    // so the source code itself contains no control chars.
    const controls = Array.from({ length: 32 }, (_, i) => String.fromCharCode(i)).join('');
    const reserved = '<>:"/\\\\|?*'; // Windows-reserved set
    const stripRe = new RegExp(`[${reserved}${controls}]`, 'g');
    const cleaned = base.replace(stripRe, '').replace(/\s+/g, ' ').trim();
    const truncated = cleaned.length > 80 ? cleaned.slice(0, 80) : cleaned;
    return `${truncated || 'kage-chat'}.md`;
}

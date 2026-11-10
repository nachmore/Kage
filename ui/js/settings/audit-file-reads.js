/**
 * Settings → Tool permissions → Audit log: the rows for files that
 * extensions read (docs/design/EXTENSION_FILE_TEXT.md §4.5, §5.13).
 * tool-permissions.js hands these entries here.
 *
 * Everything in an entry is shown as text. The file name comes from the
 * user's disk and the extension name from an untrusted manifest, and both
 * sit in `<bdi>`, so a right-to-left name can't reorder the row around it.
 */

import { cleanDisplayName } from '../shared/extension-file-text-ui.js';
import { extensionDisplay, localizeManifestForPrompt } from '../shared/extension-manager/i18n.js';
import { activeLanguage, t } from '../shared/i18n.js';
import { escapeHtml } from '../shared/tool-utils.js';

/** The audit event (src/permission_audit/types.rs `ExtensionFileRead`). */
export const FILE_READ_EVENT = 'extension_file_read';

const MAX_FILE_NAME_CHARS = 255;
const MAX_CODE_CHARS = 64;
const SIZE_UNITS = ['KB', 'MB', 'GB', 'TB'];

/** The audit filter's option for these rows ("File reads"). */
export function fileReadFilterOption() {
    return `<option value="${FILE_READ_EVENT}">${escapeHtml(t('settings.tool_permissions.audit.filter.file_read'))}</option>`;
}

/**
 * The names of the extensions that read the files in `entries`: id → name.
 * Only installed extensions are found; a row falls back to the id for the
 * rest. Asks the backend nothing when no entry is a file read. Never throws.
 *
 * @param {object[]} entries - audit entries
 * @param {Function} [invoke] - default: this window's Tauri invoke
 */
export async function loadFileReadNames(entries, invoke = globalThis.__TAURI__?.core?.invoke) {
    const names = new Map();
    const ids = new Set(
        (Array.isArray(entries) ? entries : [])
            .filter((e) => e?.event === FILE_READ_EVENT && typeof e.extension_id === 'string')
            .map((e) => e.extension_id)
    );
    if (ids.size === 0 || typeof invoke !== 'function') return names;
    let installed;
    try {
        installed = await invoke('list_extensions');
    } catch {
        return names;
    }
    const wanted = (Array.isArray(installed) ? installed : [])
        .map((item) => item?.manifest)
        .filter((m) => m && typeof m.id === 'string' && ids.has(m.id));
    await Promise.all(
        wanted.map(async (manifest) => {
            // Only a `__MSG_*__` name needs the extension's catalog.
            const localizedManifest = /^__MSG_.+__$/.test(manifest.name ?? '')
                ? await localizeManifestForPrompt(invoke, manifest)
                : manifest;
            const name = cleanDisplayName(
                extensionDisplay({ manifest, localizedManifest }, manifest.id).name
            );
            if (name) names.set(manifest.id, name);
        })
    );
    return names;
}

/**
 * One file-read row's parts, for the audit list's common row:
 * `{ icon, summary, meta }`, all safe HTML.
 *
 * @param {object} entry - an `extension_file_read` audit entry
 * @param {Map<string,string>} [names] - from {@link loadFileReadNames}
 */
export function fileReadRow(entry, names) {
    const id = typeof entry?.extension_id === 'string' ? entry.extension_id : '';
    const extension = isolate(names?.get(id) || cleanDisplayName(id, '?'));
    const name =
        typeof entry?.file_name === 'string' ? entry.file_name.slice(0, MAX_FILE_NAME_CHARS) : '';
    const file = name
        ? isolate(name)
        : escapeHtml(t('settings.tool_permissions.audit.summary.extension_file_read_unnamed'));
    const outcome = typeof entry?.outcome === 'string' ? entry.outcome : '';
    if (outcome === 'ok') {
        const format = typeof entry.format === 'string' && entry.format ? entry.format : '?';
        return {
            icon: '📄',
            summary: t('settings.tool_permissions.audit.summary.extension_file_read', {
                extension,
                file,
                format: isolate(format.slice(0, MAX_CODE_CHARS), 'ltr'),
                size: isolate(formatFileSize(entry.bytes), 'ltr'),
                chars: escapeHtml(formatCount(entry.chars)),
            }),
            meta: '',
        };
    }
    return {
        icon: '📄',
        summary: t('settings.tool_permissions.audit.summary.extension_file_read_failed', {
            extension,
            file,
        }),
        // The outcome code (`too_large`, `malformed`, …) as a muted tag.
        meta: `<code dir="ltr">${escapeHtml((outcome || '?').slice(0, MAX_CODE_CHARS))}</code>`,
    };
}

/**
 * `48.2 MB`: the size as the host's error messages write it
 * (`file_text::format_size` in Rust): binary units, one decimal, no `.0`.
 */
export function formatFileSize(bytes) {
    const n = Number.isFinite(bytes) && bytes > 0 ? Math.floor(bytes) : 0;
    if (n < 1024) return `${n} B`;
    let value = n / 1024;
    let unit = 0;
    while (value >= 1024 && unit + 1 < SIZE_UNITS.length) {
        value /= 1024;
        unit += 1;
    }
    return `${value.toFixed(1).replace(/\.0$/, '')} ${SIZE_UNITS[unit]}`;
}

/** A character count in the UI language's digits and separators. */
function formatCount(chars) {
    const n = Number.isFinite(chars) && chars > 0 ? Math.floor(chars) : 0;
    try {
        return n.toLocaleString(activeLanguage());
    } catch {
        return String(n);
    }
}

/** `text`, escaped, in a `<bdi>` (`dir` is `auto` or `ltr`). */
function isolate(text, dir = 'auto') {
    return `<bdi dir="${dir}">${escapeHtml(text)}</bdi>`;
}

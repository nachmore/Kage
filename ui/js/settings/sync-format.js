/**
 * Small text helpers shared by the Settings → Sync views
 * (docs/design/SYNC.md §13, §17): times in the user's language, user text
 * escaped and bidi-isolated, and catalog keys rendered with their params.
 * Pure, no Tauri.
 */

import { activeLanguage, t, tHtml } from '../shared/i18n.js';
import { isolate } from '../shared/sync-scope-chip.js';
import { escapeAttr, escapeHtml } from '../shared/tool-utils.js';

export { escapeAttr, escapeHtml, isolate };

/** User text (a computer or item name) for HTML: escaped and isolated. */
export function userText(text) {
    return `<bdi>${escapeHtml(text ?? '')}</bdi>`;
}

/** Text that reads left to right whatever the language: paths, commands. */
export function ltrText(text) {
    return `<span dir="ltr" class="ltr">${escapeHtml(text ?? '')}</span>`;
}

function lang() {
    try {
        return activeLanguage() || 'en';
    } catch {
        return 'en';
    }
}

/** "2 minutes ago", "yesterday"; a date after a week. */
export function relativeTime(ms, now = Date.now()) {
    if (!Number.isFinite(ms)) return '';
    const diff = ms - now;
    const abs = Math.abs(diff);
    const rtf = new Intl.RelativeTimeFormat(lang(), { numeric: 'auto' });
    if (abs < 60_000) return rtf.format(0, 'minute');
    if (abs < 3_600_000) return rtf.format(Math.round(diff / 60_000), 'minute');
    if (abs < 86_400_000) return rtf.format(Math.round(diff / 3_600_000), 'hour');
    if (abs < 7 * 86_400_000) return rtf.format(Math.round(diff / 86_400_000), 'day');
    return dateText(ms);
}

/** A date in the user's language. */
export function dateText(ms) {
    if (!Number.isFinite(ms)) return '';
    return new Intl.DateTimeFormat(lang(), { dateStyle: 'medium' }).format(new Date(ms));
}

/** A date and time in the user's language. */
export function dateTimeText(ms) {
    if (!Number.isFinite(ms)) return '';
    return new Intl.DateTimeFormat(lang(), { dateStyle: 'medium', timeStyle: 'short' }).format(
        new Date(ms)
    );
}

/** A time of day, with the date when it isn't today. */
export function shortTime(ms, now = Date.now()) {
    if (!Number.isFinite(ms)) return '';
    const d = new Date(ms);
    const sameDay = new Date(now).toDateString() === d.toDateString();
    return sameDay
        ? new Intl.DateTimeFormat(lang(), { timeStyle: 'short' }).format(d)
        : dateTimeText(ms);
}

/** Params for ICU: numeric strings become numbers, so plurals work. */
export function icuParams(params) {
    const out = {};
    for (const [k, v] of Object.entries(params || {})) {
        out[k] = typeof v === 'string' && /^-?\d+$/.test(v) ? Number(v) : v;
    }
    return out;
}

/** The Settings name of each config list, for a namespace in a label. */
const LIST_LABELS = {
    'list.shortcuts': 'settings.sidebar.shortcuts',
    'list.macros': 'settings.sidebar.macros',
    'list.quick_actions': 'settings.assistant.quick_actions.custom.label',
    'list.app_modes': 'settings.assistant.app_modes.label',
    'list.internal_sites': 'settings.sidebar.internal_sites',
    'list.connections': 'sync.label.connections',
    'list.store_sources': 'sync.label.store_sources',
};

/**
 * Params for a label: a param that is itself a catalog key (`list`) or a
 * list namespace (`what` of a bulk delete) is rendered; other text is user
 * text, isolated.
 */
function labelVars(params) {
    const vars = {};
    for (const [k, v] of Object.entries(icuParams(params))) {
        if (typeof v !== 'string') vars[k] = v;
        else if (LIST_LABELS[v]) vars[k] = t(LIST_LABELS[v]);
        else if (/^(settings|sync)\.[a-z0-9_.]+$/.test(v)) vars[k] = t(v);
        else vars[k] = isolate(v);
    }
    return vars;
}

/**
 * A `{ key, params }` label from the engine, as HTML. User text in params
 * is escaped and isolated.
 */
export function labelHtml(key, params) {
    return key ? tHtml(key, labelVars(params)) : '';
}

/** The same label as plain text (for a param of another sentence). */
export function labelText(key, params) {
    return key ? t(key, labelVars(params)) : '';
}

/** "A, B and C" in the user's language. */
export function listText(items) {
    const list = (items || []).filter(Boolean);
    if (list.length === 0) return '';
    try {
        return new Intl.ListFormat(lang(), { type: 'conjunction' }).format(list);
    } catch {
        return list.join(', ');
    }
}

/** The OS name of a wire OS. */
export function osName(os) {
    return t(`sync.os.${os === 'macos' || os === 'windows' ? os : 'linux'}`);
}

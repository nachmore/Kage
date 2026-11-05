/**
 * Small DOM helpers shared by the tool-call views (shell-command-view.js,
 * tool-call-view.js): element creation, the constant SVG icon set, copy
 * buttons, chip names, and the bubbling layout event a self-sizing window
 * listens for.
 *
 * Nothing here ever parses data as HTML: icons are constant markup, text
 * goes in via textContent.
 */

import { t } from './i18n.js';

/** Bubbling event fired from a block or chip when its height changes. */
export const TOOL_LAYOUT_EVENT = 'kage:shell-layout';

// Constant, trusted SVG markup only — never interpolate data into these.
const ICONS = Object.freeze({
    terminal: '<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>',
    check: '<polyline points="20 6 9 17 4 12"/>',
    cross: '<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>',
    dash: '<circle cx="12" cy="12" r="9"/><line x1="8" y1="12" x2="16" y2="12"/>',
    chevron: '<polyline points="9 18 15 12 9 6"/>',
    copy: '<rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>',
    edit: '<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/>',
    read: '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="8" y1="13" x2="16" y2="13"/><line x1="8" y1="17" x2="13" y2="17"/>',
    search: '<circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>',
    fetch: '<circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15 15 0 0 1 0 20a15 15 0 0 1 0-20z"/>',
    mcp: '<path d="M9 2v6"/><path d="M15 2v6"/><path d="M6 8h12v4a6 6 0 0 1-12 0z"/><path d="M12 18v4"/>',
    think: '<path d="M9 18h6"/><path d="M10 22h4"/><path d="M12 2a7 7 0 0 0-4 12.7V16h8v-1.3A7 7 0 0 0 12 2z"/>',
    other: '<path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-.6-.6-2.4z"/>',
});

export function el(tag, className) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    return node;
}

/** A decorative icon span (`aria-hidden`). Unknown names fall back to `other`. */
export function icon(name, className, size = 14) {
    const span = el('span', className || 'shell-cmd-icon');
    span.setAttribute('aria-hidden', 'true');
    const paths = ICONS[name] || ICONS.other;
    span.innerHTML = `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${paths}</svg>`;
    return span;
}

export function layoutChanged(node) {
    node.dispatchEvent(new CustomEvent(TOOL_LAYOUT_EVENT, { bubbles: true }));
}

function writeClipboard(text) {
    try {
        const p = navigator.clipboard?.writeText(text);
        return p ? p.then(() => true).catch(() => false) : Promise.resolve(false);
    } catch {
        return Promise.resolve(false);
    }
}

const COPIED_FLASH_MS = 1500;

/**
 * A "Copy …" button: `getLabel()` its (translated) label, `getText()` the
 * text it copies, read at click time. Flashes "Copied" on success.
 */
export function copyButton(getLabel, getText) {
    const btn = el('button', 'shell-cmd-copy');
    btn.type = 'button';
    const label = el('span', 'shell-cmd-copy-label');
    label.dir = 'auto';
    label.textContent = getLabel();
    btn.append(icon('copy', 'shell-cmd-icon', 12), label);
    btn.addEventListener('click', (e) => {
        e.stopPropagation();
        const text = getText();
        if (!text) return;
        writeClipboard(text).then((ok) => {
            if (!ok) return;
            label.textContent = t('shared.shell.copied');
            btn.classList.add('copied');
            clearTimeout(btn._kageCopiedTimer);
            btn._kageCopiedTimer = setTimeout(() => {
                label.textContent = getLabel();
                btn.classList.remove('copied');
            }, COPIED_FLASH_MS);
        });
    });
    return btn;
}

/**
 * A tool / shell chip's accessible name (and tooltip): its `title`, plus
 * whether a call under it failed or is still running (`state`: the chip's
 * `running` / `failed` / `done`). Its red dot is decorative; this says it.
 */
export function chipStateLabel(title, state) {
    if (state === 'failed') return t('shared.tool.chip_state.failed', { label: title });
    if (state === 'running') return t('shared.tool.chip_state.running', { label: title });
    return title;
}

let _uid = 0;

/** A document-unique id with `prefix`. */
export function uid(prefix) {
    _uid += 1;
    return `${prefix}-${_uid}`;
}

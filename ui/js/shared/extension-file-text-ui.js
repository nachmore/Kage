/**
 * What the user sees while an extension reads a file they pick
 * (docs/design/EXTENSION_FILE_TEXT.md §4.3, §4.4): the ask bar and the
 * progress chip. Host-rendered DOM, outside every extension iframe. Names
 * come from the host and are shown in `<bdi dir="auto">`, so a
 * right-to-left name can't reorder the text around it. The decisions are
 * the broker's (extension-file-text.js); this file only draws.
 */

import { t as hostT } from './i18n.js';

/** "Choose file…" ignores clicks this soon after the bar appears. */
export const ASK_BAR_ARM_MS = 500;

const MAX_DISPLAY_NAME_CHARS = 64;
const MAX_ICON_CHARS = 16;
const DEFAULT_ICON = '📄';
const LAYER_ID = 'kage-file-text-layer';
/** Stands in for a name inside a translated template (private use). */
const MARK = '\uE000';

/** Characters a name never keeps: controls, bidi controls, tags, noncharacters. */
function isDroppedChar(cp) {
    return (
        cp <= 0x1f ||
        (cp >= 0x7f && cp <= 0x9f) ||
        cp === 0xfeff ||
        cp === 0x061c ||
        cp === 0x200e ||
        cp === 0x200f ||
        (cp >= 0x202a && cp <= 0x202e) ||
        (cp >= 0x2066 && cp <= 0x2069) ||
        (cp >= 0xe0000 && cp <= 0xe007f) ||
        (cp >= 0xfdd0 && cp <= 0xfdef) ||
        (cp & 0xfffe) === 0xfffe
    );
}

/** One clean line, as Rust's `clean_line`: NFC, dropped characters, single spaces. */
function cleanLine(raw, max) {
    if (typeof raw !== 'string') return '';
    let out = '';
    let n = 0;
    let space = false;
    for (const ch of raw.normalize('NFC')) {
        if (/\s/u.test(ch)) {
            space = true;
            continue;
        }
        if (isDroppedChar(ch.codePointAt(0))) continue;
        const need = space && n > 0 ? 2 : 1;
        if (n + need > max) break;
        out += need === 2 ? ` ${ch}` : ch;
        n += need;
        space = false;
    }
    return out;
}

/** The name the dialog title and the ask bar show: cleaned, ≤ 64 characters. */
export function cleanDisplayName(raw, fallback = '') {
    return cleanLine(raw, MAX_DISPLAY_NAME_CHARS) || (typeof fallback === 'string' ? fallback : '');
}

/** The manifest icon for the ask bar, or the default. */
export function cleanDisplayIcon(raw) {
    return cleanLine(raw, MAX_ICON_CHARS) || DEFAULT_ICON;
}

function el(doc, tag, className) {
    const node = doc.createElement(tag);
    if (className) node.className = className;
    return node;
}

function button(doc, className, label) {
    const btn = el(doc, 'button', className);
    btn.type = 'button';
    btn.textContent = label;
    return btn;
}

/** A name that can't reorder the text around it. */
function isolated(doc, text) {
    const bdi = el(doc, 'bdi');
    bdi.dir = 'auto';
    bdi.textContent = text;
    return bdi;
}

/** Append a translated template, with `node` where its {@link MARK} was. */
function appendTemplate(parent, template, node) {
    const text = String(template);
    const at = text.indexOf(MARK);
    if (at < 0) {
        parent.append(text, ' ', node);
        return;
    }
    parent.append(text.slice(0, at), node, text.slice(at + MARK.length).replaceAll(MARK, ''));
}

let askSeq = 0;

/**
 * Draws the ask bar and the chip in one window.
 *
 * @param {object} [opts]
 * @param {Element|(() => Element|null)} [opts.mount] - where they go, at the
 *   top; default a fixed layer at the top of the window
 * @param {() => void} [opts.onLayoutChange] - the bar or chip came or went
 * @param {Function} [opts.t] - the translator; default the host's
 */
export class FileTextSurface {
    constructor(opts = {}) {
        this._mount = opts.mount ?? null;
        this._onLayout = typeof opts.onLayoutChange === 'function' ? opts.onLayoutChange : null;
        this._t = typeof opts.t === 'function' ? opts.t : hostT;
    }

    /**
     * Show the ask bar for an extension. `answer` resolves 'choose',
     * 'dismiss' (Not now or Escape) or 'timeout' (after `timeoutMs`);
     * `settle(answer)` closes the bar from outside with any answer. Focus
     * moves to Choose file… and goes back where it was when the bar closes.
     *
     * @param {{name: string, icon?: string, timeoutMs: number}} who
     * @returns {{answer: Promise<string>, settle: (answer: string) => void}}
     */
    showAsk({ name, icon, timeoutMs }) {
        const doc = globalThis.document;
        const t = this._t;
        const bar = el(doc, 'div', 'kage-file-ask');
        bar.setAttribute('role', 'alertdialog');
        const msgId = `kage-file-ask-msg-${++askSeq}`;
        bar.setAttribute('aria-labelledby', msgId);
        const iconEl = el(doc, 'span', 'kage-file-ask-icon');
        iconEl.setAttribute('aria-hidden', 'true');
        iconEl.textContent = cleanDisplayIcon(icon);
        const text = el(doc, 'span', 'kage-file-ask-text');
        text.id = msgId;
        const strong = el(doc, 'strong');
        strong.append(isolated(doc, name));
        appendTemplate(text, t('file_text.ask.message', { extension: MARK }), strong);
        const actions = el(doc, 'span', 'kage-file-ask-actions');
        const choose = button(doc, 'kage-file-ask-choose', t('file_text.ask.choose'));
        const dismiss = button(doc, 'kage-file-ask-dismiss', t('file_text.ask.dismiss'));
        actions.append(choose, dismiss);
        bar.append(iconEl, text, actions);

        const before = doc.activeElement;
        let resolve;
        const answer = new Promise((r) => {
            resolve = r;
        });
        let open = true;
        // A bar that appears under a click meant for something else must
        // not take it (the layout above it moved), nor an Enter typed
        // into the input it took focus from.
        let armed = false;
        choose.setAttribute('aria-disabled', 'true');
        const armTimer = setTimeout(() => {
            armed = true;
            choose.removeAttribute('aria-disabled');
        }, ASK_BAR_ARM_MS);
        const timeout = setTimeout(() => settle('timeout'), timeoutMs);
        const settle = (result) => {
            if (!open) return;
            open = false;
            clearTimeout(armTimer);
            clearTimeout(timeout);
            const hadFocus = bar.contains(doc.activeElement);
            this._unplace(bar);
            if (hadFocus && before?.isConnected && typeof before.focus === 'function') {
                try {
                    before.focus({ preventScroll: true });
                } catch {}
            }
            resolve(result);
        };
        choose.addEventListener('click', (ev) => {
            ev.preventDefault();
            if (armed) settle('choose');
        });
        dismiss.addEventListener('click', (ev) => {
            ev.preventDefault();
            settle('dismiss');
        });
        bar.addEventListener('keydown', (ev) => {
            if (ev.key !== 'Escape') return;
            ev.preventDefault();
            ev.stopPropagation();
            settle('dismiss');
        });
        this._place(bar);
        try {
            choose.focus({ preventScroll: true });
        } catch {}
        return { answer, settle };
    }

    /**
     * Show the progress chip (§4.4). `paint(phase, file)` sets its text:
     * "Downloading {file}…" for `downloading`, else "Reading {file}…".
     * Cancel calls `onCancel` once.
     *
     * @param {{onCancel: () => void}} hooks
     * @returns {{paint: (phase: string, file: string) => void, remove: () => void}}
     */
    showChip({ onCancel }) {
        const doc = globalThis.document;
        const chip = el(doc, 'div', 'kage-file-chip');
        chip.setAttribute('role', 'status');
        const label = el(doc, 'span', 'kage-file-chip-text');
        const cancel = button(doc, 'kage-file-chip-cancel', this._t('file_text.progress.cancel'));
        cancel.addEventListener('click', (ev) => {
            ev.preventDefault();
            if (cancel.disabled) return;
            cancel.disabled = true;
            onCancel();
        });
        chip.append(label, cancel);
        const paint = (phase, file) => {
            const key =
                phase === 'downloading'
                    ? 'file_text.progress.downloading'
                    : 'file_text.progress.reading';
            label.replaceChildren();
            appendTemplate(label, this._t(key, { file: MARK }), isolated(doc, file));
        };
        this._place(chip);
        return { paint, remove: () => this._unplace(chip) };
    }

    /** Where the bar and chip go: the window's mount, else the fixed layer. */
    _host() {
        let mount = null;
        try {
            mount = typeof this._mount === 'function' ? this._mount() : this._mount;
        } catch {}
        if (mount?.isConnected) return mount;
        const doc = globalThis.document;
        let layer = doc.getElementById(LAYER_ID);
        if (!layer) {
            layer = el(doc, 'div', 'kage-file-text-layer');
            layer.id = LAYER_ID;
            doc.body.appendChild(layer);
        }
        return layer;
    }

    _place(node) {
        const host = this._host();
        host.insertBefore(node, host.firstChild);
        this._layout();
    }

    _unplace(node) {
        if (!node.isConnected) return;
        node.remove();
        this._layout();
    }

    _layout() {
        try {
            this._onLayout?.();
        } catch (e) {
            console.warn('[file-text] layout hook failed:', e);
        }
    }
}

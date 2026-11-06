/**
 * Popup menu window — rendering and keyboard/mouse behaviour, kept free of
 * Tauri so it can be tested (ui/js/context-menu/main.js wires it up).
 *
 * Native-menu conventions: ↑/↓ move the highlight (wrapping, skipping
 * separators and disabled items), Home/End jump, Enter/Space choose, Esc
 * dismisses, hovering highlights, and only a primary-button press chooses.
 * A keyboard-opened menu starts with the first item highlighted; a
 * mouse-opened one with none.
 */

import { firstActionable, isActionable, lastActionable, stepHighlight } from './popup-menu.js';

/**
 * Render `items` into `root` (replacing its contents). Labels come from
 * `t(labelKey, labelVars)` or the literal `label`, always as text.
 */
export function renderMenuItems(root, items, t) {
    root.textContent = '';
    items.forEach((item, index) => {
        if (item.separator) {
            const sep = document.createElement('div');
            sep.className = 'menu-sep';
            sep.setAttribute('role', 'separator');
            root.appendChild(sep);
            return;
        }
        const el = document.createElement('div');
        el.className = 'menu-item';
        el.dataset.index = String(index);
        el.dataset.action = item.id;
        el.setAttribute('role', 'menuitem');
        el.tabIndex = -1;
        if (item.danger) el.classList.add('danger');
        if (item.enabled === false) {
            el.classList.add('disabled');
            el.setAttribute('aria-disabled', 'true');
        }
        const label = document.createElement('span');
        label.className = 'menu-label';
        label.textContent = item.labelKey ? t(item.labelKey, item.labelVars || {}) : item.label;
        el.appendChild(label);
        if (item.shortcut) {
            const hint = document.createElement('span');
            hint.className = 'menu-shortcut';
            // Key names read left-to-right in every language.
            hint.setAttribute('dir', 'ltr');
            hint.textContent = item.shortcut;
            el.appendChild(hint);
        }
        root.appendChild(el);
    });
}

/**
 * A primary-button press: not the right or middle button, and on macOS
 * not Ctrl+click (a right click there).
 */
export function isPrimaryClick(e, { mac = false } = {}) {
    return !!e && e.button === 0 && !(mac && e.ctrlKey);
}

export class PopupMenuView {
    /**
     * @param {HTMLElement} root   the `.menu` container
     * @param {object} opts
     * @param {(key: string, vars?: object) => string} opts.t
     * @param {(id: string) => void} opts.onChoose
     * @param {() => void} opts.onDismiss
     * @param {boolean} [opts.mac] Ctrl+click is a right click
     */
    constructor(root, { t, onChoose, onDismiss, mac = false }) {
        this.root = root;
        this.t = t;
        this.mac = !!mac;
        this.onChoose = onChoose;
        this.onDismiss = onDismiss;
        this.items = [];
        this.highlighted = -1;
        root.setAttribute('role', 'menu');

        root.addEventListener('mousemove', (e) => {
            const el = e.target.closest?.('.menu-item');
            if (!el) return;
            const i = Number(el.dataset.index);
            if (i !== this.highlighted && isActionable(this.items[i])) this.highlight(i);
        });
        root.addEventListener('mouseleave', () => this.highlight(-1));
        // mousedown, not click: the window may lose focus (and dismiss)
        // before a click completes. Only the primary button chooses — a
        // right or middle click on an item must not run it (and macOS
        // Ctrl+click is a right click).
        root.addEventListener('mousedown', (e) => {
            const el = e.target.closest?.('.menu-item');
            if (!el) return;
            e.preventDefault();
            if (!isPrimaryClick(e, { mac: this.mac })) return;
            this.choose(Number(el.dataset.index));
        });
    }

    /** Show `items`; returns the rendered menu's size in CSS px. */
    show(items, { keyboard = false } = {}) {
        this.items = items;
        renderMenuItems(this.root, items, this.t);
        this.highlight(keyboard ? firstActionable(items) : -1);
        const rect = this.root.getBoundingClientRect();
        return { width: Math.ceil(rect.width), height: Math.ceil(rect.height) };
    }

    highlight(index) {
        this.highlighted = index;
        for (const el of this.root.querySelectorAll('.menu-item')) {
            const on = Number(el.dataset.index) === index;
            el.classList.toggle('highlighted', on);
            if (on) {
                el.setAttribute('aria-selected', 'true');
                el.focus?.({ preventScroll: true });
            } else {
                el.removeAttribute('aria-selected');
            }
        }
    }

    choose(index) {
        const item = this.items[index];
        if (isActionable(item)) this.onChoose(item.id);
    }

    /** Handle a keydown; returns true if it was a menu key. */
    handleKey(e) {
        switch (e.key) {
            case 'ArrowDown':
                this.highlight(stepHighlight(this.items, this.highlighted, 1));
                break;
            case 'ArrowUp':
                this.highlight(stepHighlight(this.items, this.highlighted, -1));
                break;
            case 'Home':
            case 'PageUp':
                this.highlight(firstActionable(this.items));
                break;
            case 'End':
            case 'PageDown':
                this.highlight(lastActionable(this.items));
                break;
            case 'Enter':
            case ' ':
                if (this.highlighted >= 0) this.choose(this.highlighted);
                break;
            case 'Escape':
                this.onDismiss();
                break;
            case 'Tab':
                // Native menus keep focus; don't let it leave the menu.
                break;
            default:
                return false;
        }
        e.preventDefault();
        return true;
    }
}

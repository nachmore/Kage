/**
 * LineEditor — line-oriented document editor for the steering docs
 * (auto-generated preferences + user steering).
 *
 * The model is intentionally *not* a freeform textarea: each line of
 * the document is a row that the user can reorder, edit, or delete.
 * That maps well onto how the auto-generated preferences doc is
 * structured (one fact per line) and prevents accidental mass edits
 * when the user only meant to tweak a single bullet.
 *
 * Usage:
 *   import { createLineEditor } from '../shared/line-editor.js';
 *   const editor = createLineEditor(container, {
 *       lines: ['## Heading', '- one', '- two'],
 *       emptyHint: 'No preferences yet.',
 *   });
 *   const updated = editor.getLines();   // current state
 *   editor.setLines(['fresh', 'lines']); // replace contents
 *
 * Behaviours:
 *   - Each row is a single-line input so paste-of-multi-line is split
 *     into N rows automatically (intuitive for moving bullets around).
 *   - Up/down buttons reorder; ✕ deletes; "+ Add line" appends.
 *   - Empty rows are kept (they map to paragraph breaks in markdown).
 *   - Keyboard: Enter on a row inserts a new row below; Backspace on
 *     an empty row deletes it and focuses the previous row.
 *
 * Styling lives in shared-components.css under `.line-editor*`.
 *
 * Tested via ui-tests/shared/line-editor.test.js. The tests use
 * jsdom — keep the DOM API minimal so the tests don't need to mock
 * paint geometry.
 */

const NS = 'line-editor';

function escapeAttr(s) {
    return String(s).replace(/[&<>"']/g, (c) => {
        return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
}

/**
 * Create a line editor inside `container`. Returns a controller object
 * with `getLines`, `setLines`, `destroy`, and `focusLast`.
 */
export function createLineEditor(container, opts = {}) {
    const { lines = [], emptyHint = '', addLabel = '+ Add line', rowPlaceholder = '' } = opts;

    if (!container) throw new Error('createLineEditor: container is required');

    const root = document.createElement('div');
    root.className = `${NS}-root`;

    const list = document.createElement('div');
    list.className = `${NS}-list`;
    root.appendChild(list);

    const empty = document.createElement('div');
    empty.className = `${NS}-empty`;
    empty.textContent = emptyHint;
    if (!emptyHint) empty.style.display = 'none';
    root.appendChild(empty);

    const addBtn = document.createElement('button');
    addBtn.type = 'button';
    addBtn.className = `setting-button ${NS}-add`;
    addBtn.textContent = addLabel;
    root.appendChild(addBtn);

    container.innerHTML = '';
    container.appendChild(root);

    function syncEmptyHint() {
        if (!emptyHint) return;
        const hasRows = list.querySelector(`.${NS}-row`) !== null;
        empty.style.display = hasRows ? 'none' : '';
    }

    function makeRow(text) {
        const row = document.createElement('div');
        row.className = `${NS}-row`;
        row.innerHTML = `
            <button type="button" class="${NS}-btn ${NS}-up" title="Move up" aria-label="Move line up">▲</button>
            <button type="button" class="${NS}-btn ${NS}-down" title="Move down" aria-label="Move line down">▼</button>
            <input type="text" class="${NS}-input setting-input" value="${escapeAttr(text)}" placeholder="${escapeAttr(rowPlaceholder)}" spellcheck="false">
            <button type="button" class="${NS}-btn ${NS}-remove" title="Delete line" aria-label="Delete line">✕</button>
        `;
        return row;
    }

    function rows() {
        return [...list.querySelectorAll(`.${NS}-row`)];
    }

    function focusInput(row) {
        const input = row?.querySelector(`.${NS}-input`);
        if (input) input.focus();
    }

    function insertRowAt(index, text) {
        const row = makeRow(text);
        const all = rows();
        if (index >= 0 && index < all.length) {
            list.insertBefore(row, all[index]);
        } else {
            list.appendChild(row);
        }
        syncEmptyHint();
        return row;
    }

    function appendRow(text) {
        return insertRowAt(rows().length, text);
    }

    function setAll(values) {
        list.innerHTML = '';
        // The split below maps multiline paste to row-per-line so
        // the user can drop a chunk of markdown and it Just Works.
        const flattened = [];
        for (const v of values) {
            if (typeof v !== 'string') continue;
            const split = v.split('\n');
            for (const piece of split) flattened.push(piece);
        }
        for (const text of flattened) appendRow(text);
        syncEmptyHint();
    }

    setAll(lines);

    // --- event delegation -------------------------------------------------
    //
    // One click handler walks to the nearest row and dispatches by class on
    // the source button. Cheaper than wiring per-row listeners + survives
    // adds/removes without re-binding.
    list.addEventListener('click', (event) => {
        const btn = event.target.closest(`.${NS}-btn`);
        if (!btn) return;
        const row = btn.closest(`.${NS}-row`);
        if (!row) return;
        const all = rows();
        const idx = all.indexOf(row);

        if (btn.classList.contains(`${NS}-up`)) {
            if (idx > 0) {
                list.insertBefore(row, all[idx - 1]);
                focusInput(row);
            }
        } else if (btn.classList.contains(`${NS}-down`)) {
            if (idx < all.length - 1) {
                list.insertBefore(all[idx + 1], row);
                focusInput(row);
            }
        } else if (btn.classList.contains(`${NS}-remove`)) {
            const fallback = all[idx + 1] || all[idx - 1] || null;
            row.remove();
            syncEmptyHint();
            if (fallback) focusInput(fallback);
        }
    });

    // Keyboard niceties: Enter to add a new row below, Backspace on an
    // already-empty row deletes it and steps focus back. These match the
    // intuitions of every line-list editor we tested against (Notion's
    // bullet list, Apple Notes, Trello card lines).
    list.addEventListener('keydown', (event) => {
        const input = event.target.closest(`.${NS}-input`);
        if (!input) return;
        const row = input.closest(`.${NS}-row`);
        const all = rows();
        const idx = all.indexOf(row);

        if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault();
            const newRow = insertRowAt(idx + 1, '');
            focusInput(newRow);
        } else if (event.key === 'Backspace' && input.value === '' && all.length > 1) {
            event.preventDefault();
            const fallback = all[idx - 1] || all[idx + 1] || null;
            row.remove();
            syncEmptyHint();
            if (fallback) focusInput(fallback);
        }
    });

    addBtn.addEventListener('click', () => {
        const newRow = appendRow('');
        focusInput(newRow);
    });

    // --- controller surface ----------------------------------------------

    return {
        /** Snapshot the current line list, in the user's chosen order. */
        getLines() {
            return rows().map((row) => row.querySelector(`.${NS}-input`)?.value ?? '');
        },
        /** Replace the contents wholesale. Convenience for "Import". */
        setLines(values) {
            setAll(Array.isArray(values) ? values : []);
        },
        /** Append a single line and focus it. Useful for paste handlers. */
        appendLine(text = '') {
            const row = appendRow(text);
            focusInput(row);
            return row;
        },
        /** Focus the trailing input. Used right after `setLines`. */
        focusLast() {
            const all = rows();
            if (all.length) focusInput(all[all.length - 1]);
        },
        /** Drop event listeners + clear the container for re-use. */
        destroy() {
            container.innerHTML = '';
        },
        /** Surface the root element for layout-sensitive callers. */
        get element() {
            return root;
        },
    };
}

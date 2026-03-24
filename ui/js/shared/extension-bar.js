/**
 * Extension Bar API — shared utility for mounting persistent bars above the
 * floating window input. Any extension can use this to display status bars
 * that are automatically accounted for in the window resize logic.
 *
 * All bars use the `.extension-bar` CSS class (defined in floating-components.css)
 * which the window manager queries for height calculations.
 *
 * Usage:
 *   import { showExtensionBar, updateExtensionBar, hideExtensionBar } from '../shared/extension-bar.js';
 *
 *   showExtensionBar({
 *       id: 'my-bar',
 *       icon: '🔔',
 *       text: 'Something happened',
 *       buttons: [
 *           { id: 'action', label: 'Do it', title: 'Do the thing', onClick: () => {} },
 *           { id: 'dismiss', label: '✕', title: 'Dismiss', onClick: () => hideExtensionBar('my-bar') },
 *       ],
 *   });
 *
 *   updateExtensionBar('my-bar', { text: 'Updated text', counter: '2/5' });
 *   hideExtensionBar('my-bar');
 */

/**
 * Show (or replace) an extension bar above the input container.
 * @param {object} opts
 * @param {string} opts.id          Unique bar ID
 * @param {string} opts.icon        Emoji or short text for the left icon
 * @param {string} opts.text        Main text content
 * @param {string} [opts.counter]   Optional counter text (e.g. "1/3")
 * @param {Array}  [opts.buttons]   Array of { id, label, title, onClick }
 * @param {string} [opts.className] Optional extra CSS class for custom styling
 * @returns {HTMLElement|null}       The bar element, or null if input container not found
 */
export function showExtensionBar(opts) {
    const barId = `extBar_${opts.id}`;
    let bar = document.getElementById(barId);
    if (bar) bar.remove();

    const inputContainer = document.querySelector('.input-container');
    if (!inputContainer) return null;

    bar = document.createElement('div');
    bar.id = barId;
    bar.className = 'extension-bar' + (opts.className ? ` ${opts.className}` : '');

    const buttonsHtml = (opts.buttons || []).map(b =>
        `<button class="extension-bar-btn" id="${barId}_${b.id}" title="${b.title || ''}">${b.label}</button>`
    ).join('');

    const counterHtml = opts.counter
        ? `<span class="extension-bar-counter" id="${barId}_counter">${opts.counter}</span>`
        : '';

    bar.innerHTML = `
        <span class="extension-bar-icon">${opts.icon || ''}</span>
        <span class="extension-bar-text" id="${barId}_text" style="flex:1;font-size:13px;font-weight:normal;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${opts.text || ''}</span>
        <div class="extension-bar-controls">
            ${counterHtml}
            ${buttonsHtml}
        </div>
    `;

    // Prevent buttons from stealing focus (which triggers window blur → hide)
    bar.querySelectorAll('button').forEach(btn => {
        btn.addEventListener('mousedown', e => e.preventDefault());
    });

    // Wire up button click handlers
    if (opts.buttons) {
        for (const b of opts.buttons) {
            const el = document.getElementById(`${barId}_${b.id}`);
            if (el && b.onClick) el.onclick = b.onClick;
        }
    }

    inputContainer.parentNode.insertBefore(bar, inputContainer);
    bar.style.display = 'flex';

    // Request window resize to account for the new bar
    requestAnimationFrame(() => {
        document.dispatchEvent(new CustomEvent('kiro-resize-request'));
    });

    return bar;
}

/**
 * Update an existing extension bar's text and/or counter.
 * @param {string} id      The bar ID (same as passed to showExtensionBar)
 * @param {object} updates  { text?, counter?, icon? }
 */
export function updateExtensionBar(id, updates) {
    const barId = `extBar_${id}`;
    if (updates.text !== undefined) {
        const el = document.getElementById(`${barId}_text`);
        if (el) el.textContent = updates.text;
    }
    if (updates.counter !== undefined) {
        const el = document.getElementById(`${barId}_counter`);
        if (el) el.textContent = updates.counter;
    }
    if (updates.icon !== undefined) {
        const bar = document.getElementById(barId);
        if (bar) {
            const iconEl = bar.querySelector('.extension-bar-icon');
            if (iconEl) iconEl.textContent = updates.icon;
        }
    }
}

/**
 * Hide and remove an extension bar.
 * @param {string} id  The bar ID
 */
export function hideExtensionBar(id) {
    const bar = document.getElementById(`extBar_${id}`);
    if (bar) {
        bar.style.display = 'none';
        bar.remove();
        document.dispatchEvent(new CustomEvent('kiro-resize-request'));
    }
}

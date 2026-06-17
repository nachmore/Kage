/**
 * Floating window search — re-exports shared search engine + floating-specific renderer.
 */

export {
    unifiedSearch,
    recordSelection,
    loadFrecency,
    setExtensionManager,
    getExtensionManager,
} from '../shared/search-engine.js';

// --- Floating-specific suggestion renderer ---

import { escapeHtml } from '../shared/tool-utils.js';
import { getExtensionManager } from '../shared/search-engine.js';

/**
 * Build a unique key for a result so we can diff the list.
 */
function _resultKey(r) {
    // Prefer stable ID for diffing (extensions provide unique IDs)
    if (r.id) return r.id;
    if (r.type === 'app') return `app:${r.data?.name || r.label}`;
    if (r.type === 'shortcut') return `sc:${r.label}`;
    if (r.type === 'path') return `path:${r.data?.value || r.description || r.label}`;
    if (r.type === 'url') return `url:${r.data?.value || r.description || r.label}`;
    return `${r.type || 'other'}:${r.label}`;
}

/**
 * Render a single result item into a DOM element.
 */
function _renderItem(r, _index, extMgr) {
    const item = document.createElement('div');
    item.className = 'app-suggestion-item';
    item.dataset.resultKey = _resultKey(r);

    if (r._extensionId && extMgr) {
        const customEl = document.createElement('div');
        customEl.style.cssText = 'display:flex;align-items:center;gap:8px;flex:1;';
        if (extMgr.renderResult(r, customEl)) {
            item.appendChild(customEl);
            if (r.tooltip) item.title = r.tooltip;
            return item;
        }
    }

    let iconHtml;
    if (r.type === 'app' && r.data?.icon_base64) {
        const src = r.data.icon_base64.startsWith('data:')
            ? r.data.icon_base64
            : 'data:image/png;base64,' + r.data.icon_base64;
        iconHtml = `<img src="${src}" class="app-icon-img" onerror="this.style.display='none';this.nextElementSibling.style.display='flex'"><div class="app-icon" style="display:none">${r.data.emoji_icon || r.label.charAt(0).toUpperCase()}</div>`;
    } else if (r.icon?.startsWith('data:')) {
        const dot = r.type === 'window' ? '<span class="window-indicator"></span>' : '';
        iconHtml = `<div class="app-icon-wrap"><img src="${r.icon}" class="app-icon-img" style="width:24px;height:24px;border-radius:4px;object-fit:cover;">${dot}</div>`;
    } else {
        const dot = r.type === 'window' ? '<span class="window-indicator"></span>' : '';
        iconHtml = `<div class="app-icon-wrap"><div class="app-icon">${r.icon || r.label.charAt(0)}</div>${dot}</div>`;
    }

    item.innerHTML = `
        ${iconHtml}
        <div class="app-info">
            <div class="app-name">${escapeHtml(r.label)}</div>
            ${r.description ? `<div class="app-description">${escapeHtml(r.description)}</div>` : ''}
        </div>
    `;
    if (r.tooltip) item.title = r.tooltip;
    return item;
}

/**
 * Render unified search results with smooth diffing.
 * Reuses existing DOM nodes when possible, animates additions/removals.
 *
 * Now async: extensions can contribute custom render HTML, which we
 * prefetch from the sandbox before building DOM so `renderResult()` can
 * stay synchronous.
 */
export async function renderUnifiedResults(
    results,
    container,
    currentMatches,
    resizeWindow,
    onItemClick
) {
    currentMatches.length = 0;

    // Flex-collapse guard: when the suggestions dropdown becomes visible
    // inside the speech-bubble's flex column, `.content-area` (the only
    // flex:1 child) is what gives back space. Because `.content-area` has
    // overflow-y: auto, its default min-height resolves to 0 per the flex
    // spec — so the instant the dropdown appears (before the OS window has
    // grown to fit the new total), content-area can collapse to zero.
    // The calendar bar + input bar visually "jump up over the response"
    // for ~150-250ms until the OS window animation catches up and releases
    // the pressure. Lock content-area's current height during the transient
    // and release it after the resize has had time to complete.
    const contentArea = document.getElementById('contentArea');
    const willToggleVisible = results.length > 0 && !container.classList.contains('visible');
    if (willToggleVisible && contentArea?.classList.contains('visible')) {
        const h = contentArea.offsetHeight;
        if (h > 0) {
            contentArea.style.minHeight = h + 'px';
            contentArea.dataset.kageHeightLocked = '1';
        }
    }

    if (!results.length) {
        container.innerHTML = '';
        container.classList.remove('visible');
        _releaseContentLock();
        resizeWindow();
        return -1;
    }

    const extMgr = getExtensionManager();
    // Prime the custom-render cache so renderResult() below is
    // synchronous. If the sandbox isn't ready or the extension doesn't
    // implement renderCustom, this is a no-op.
    if (extMgr?.prefetchCustomRender) {
        try {
            await extMgr.prefetchCustomRender(results);
        } catch {}
    }
    const newKeys = results.map((r) => _resultKey(r));

    // Build map of existing items by key
    const existingByKey = new Map();
    container.querySelectorAll('.app-suggestion-item').forEach((el) => {
        existingByKey.set(el.dataset.resultKey, el);
    });

    // Build new item list, reusing existing DOM nodes where keys match
    const newItems = [];
    for (let i = 0; i < results.length; i++) {
        const r = results[i];
        const key = newKeys[i];
        currentMatches.push(r);

        let item = existingByKey.get(key);
        if (item) {
            // Reuse existing DOM node — update content
            existingByKey.delete(key);
            let updated = false;
            if (r._extensionId && extMgr) {
                const customEl = document.createElement('div');
                customEl.style.cssText = 'display:flex;align-items:center;gap:8px;flex:1;';
                if (extMgr.renderResult(r, customEl)) {
                    item.innerHTML = '';
                    item.appendChild(customEl);
                    updated = true;
                }
            }
            // If no custom renderer handled it, update text if it changed
            if (!updated) {
                const nameEl = item.querySelector('.app-name');
                if (nameEl && nameEl.textContent !== r.label) {
                    nameEl.textContent = r.label;
                }
                const descEl = item.querySelector('.app-description');
                if (descEl && r.description && descEl.textContent !== r.description) {
                    descEl.textContent = r.description;
                }
            }
        } else {
            // New item — create and animate in
            item = _renderItem(r, i, extMgr);
            item.style.opacity = '0';
            item.style.transform = 'translateY(4px)';
            // Trigger animation after append
            requestAnimationFrame(() =>
                requestAnimationFrame(() => {
                    item.style.opacity = '1';
                    item.style.transform = 'translateY(0)';
                })
            );
        }

        item.classList.toggle('selected', i === 0);
        // Wire click-to-execute. Use direct .onclick assignment (not
        // addEventListener) so reused DOM nodes don't stack duplicate
        // handlers across re-renders. Capture the result by value; the
        // handler mirrors what Enter on this row would do.
        if (onItemClick) {
            const captured = r;
            item.onclick = () => onItemClick(captured);
        }
        newItems.push(item);
    }

    // Remove items that are no longer in results
    for (const [, el] of existingByKey) {
        el.remove();
    }

    // Reorder: append items in the correct order
    // Only move items that are out of position to minimize DOM thrashing
    for (let i = 0; i < newItems.length; i++) {
        const item = newItems[i];
        if (container.children[i] !== item) {
            if (i < container.children.length) {
                container.insertBefore(item, container.children[i]);
            } else {
                container.appendChild(item);
            }
        }
    }

    container.classList.add('visible');
    container.scrollTop = 0;
    resizeWindow();
    // Release the content-area min-height lock after the OS window has had
    // time to resize (debounce ~50ms + animation up to ~120ms + slack).
    _scheduleContentLockRelease();
    return 0;
}

// --- Content-area min-height lock helpers (flex-collapse guard) ---

let _releaseTimer = null;

function _scheduleContentLockRelease() {
    if (_releaseTimer) clearTimeout(_releaseTimer);
    _releaseTimer = setTimeout(() => {
        _releaseTimer = null;
        _releaseContentLock();
    }, 250);
}

function _releaseContentLock() {
    if (_releaseTimer) {
        clearTimeout(_releaseTimer);
        _releaseTimer = null;
    }
    const contentArea = document.getElementById('contentArea');
    if (contentArea?.dataset.kageHeightLocked) {
        contentArea.style.minHeight = '';
        delete contentArea.dataset.kageHeightLocked;
    }
}

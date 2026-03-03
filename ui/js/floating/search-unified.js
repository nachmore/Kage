/**
 * Floating window search — re-exports shared search engine + floating-specific renderer.
 */

export { unifiedSearch, recordSelection, loadFrecency, setExtensionManager, getExtensionManager } from '../shared/search-engine.js';

// --- Floating-specific suggestion renderer ---

function _escapeHtml(str) {
    return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

/**
 * Render unified search results into the floating suggestion container.
 * Returns the selectedIndex (0 if results exist, -1 if empty).
 */
import { getExtensionManager } from '../shared/search-engine.js';

export function renderUnifiedResults(results, container, currentMatches, resizeWindow) {
    container.innerHTML = '';
    container.scrollTop = 0;
    currentMatches.length = 0;

    if (!results.length) {
        container.classList.remove('visible');
        return -1;
    }

    const extMgr = getExtensionManager();

    for (let i = 0; i < results.length; i++) {
        const r = results[i];
        currentMatches.push(r);

        const item = document.createElement('div');
        item.className = 'app-suggestion-item' + (i === 0 ? ' selected' : '');

        // Let extensions render their own results
        if (r._extensionId && extMgr) {
            const customEl = document.createElement('div');
            customEl.style.cssText = 'display:flex;align-items:center;gap:8px;flex:1;';
            if (extMgr.renderResult(r, customEl)) {
                item.appendChild(customEl);
                container.appendChild(item);
                continue;
            }
        }

        // Default rendering for non-extension results
        let iconHtml;
        if (r.type === 'app' && r.data?.icon_base64) {
            const src = r.data.icon_base64.startsWith('data:') ? r.data.icon_base64 : 'data:image/png;base64,' + r.data.icon_base64;
            iconHtml = `<img src="${src}" class="app-icon-img" onerror="this.style.display='none';this.nextElementSibling.style.display='flex'"><div class="app-icon" style="display:none">${r.data.emoji_icon || r.label.charAt(0).toUpperCase()}</div>`;
        } else {
            iconHtml = `<div class="app-icon">${r.icon || r.label.charAt(0)}</div>`;
        }

        item.innerHTML = `
            ${iconHtml}
            <div class="app-info">
                <div class="app-name">${_escapeHtml(r.label)}</div>
                ${r.description ? `<div class="app-description">${_escapeHtml(r.description)}</div>` : ''}
            </div>
        `;

        container.appendChild(item);
    }

    container.classList.add('visible');
    setTimeout(() => resizeWindow(), 10);
    return 0;
}

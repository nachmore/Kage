// App suggestions and search functionality

import { platformKeyLabel } from '../shared/shortcuts.js';
import { t, tHtml } from '../shared/i18n.js';

function shortcutIconHtml(shortcut) {
    if (shortcut.icon?.startsWith('data:')) {
        return `<img src="${shortcut.icon}" class="app-icon-img" style="width:24px;height:24px;border-radius:4px;object-fit:cover;">`;
    }
    return `<div class="app-icon">${shortcut.icon || '⚡'}</div>`;
}

export function renderShortcutSuggestion(
    shortcut,
    args,
    appSuggestions,
    currentMatches,
    executeShortcut,
    resizeWindow
) {
    appSuggestions.innerHTML = '';
    appSuggestions.scrollTop = 0;
    currentMatches.length = 0;
    currentMatches.push({ type: 'shortcut', shortcut, args });

    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';

    item.innerHTML = `
        ${shortcutIconHtml(shortcut)}
        <div class="app-name">${shortcut.name}</div>
    `;
    item.addEventListener('click', async () => await executeShortcut());

    appSuggestions.appendChild(item);
    appSuggestions.classList.add('visible');
    resizeWindow();

    return 0; // selectedIndex
}

export function renderShortcutSuggestions(
    matches,
    appSuggestions,
    selectedIndex,
    executeShortcut,
    resizeWindow
) {
    console.log('Rendering multiple shortcut suggestions:', matches);
    appSuggestions.innerHTML = '';
    appSuggestions.scrollTop = 0;

    matches.forEach((match, index) => {
        const item = document.createElement('div');
        item.className = 'app-suggestion-item';
        if (index === selectedIndex) {
            item.classList.add('selected');
        }

        const actionType = match.shortcut.action_type || 'run_program';
        const actionIcon = actionType === 'open_url' ? '🌐' : '▶️';

        item.innerHTML = `
            ${shortcutIconHtml(match.shortcut)}
            <div class="app-info">
                <div class="app-name">${match.shortcut.name}</div>
                <div class="app-description">${actionIcon} ${match.shortcut.shortcut}</div>
            </div>
        `;

        item.addEventListener('click', async () => await executeShortcut(match));
        appSuggestions.appendChild(item);
    });

    appSuggestions.classList.add('visible');
    resizeWindow();
}

export function renderUrlSuggestion(url, appSuggestions, currentMatches, openUrl, resizeWindow) {
    appSuggestions.innerHTML = '';
    appSuggestions.scrollTop = 0;
    currentMatches.length = 0;
    currentMatches.push({ type: 'url', value: url });

    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';
    item.innerHTML = `
        <div class="app-icon">🌐</div>
        <div class="app-name">${t('floating.suggestions.url.open_in_browser')}</div>
    `;
    item.addEventListener('click', async () => await openUrl(url));

    appSuggestions.appendChild(item);
    appSuggestions.classList.add('visible');
    resizeWindow();

    return 0; // selectedIndex
}

export function renderPathSuggestion(
    type,
    path,
    appSuggestions,
    currentMatches,
    openPath,
    resizeWindow
) {
    appSuggestions.innerHTML = '';
    appSuggestions.scrollTop = 0;
    currentMatches.length = 0;
    currentMatches.push({ type: 'path', value: path, pathType: type });

    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';

    const icon = type === 'file' ? '📄' : '📁';
    const label =
        type === 'file'
            ? t('floating.suggestions.path.open_file', { path })
            : t('floating.suggestions.path.open_folder', { path });

    item.innerHTML = `
        <div class="app-icon">${icon}</div>
        <div class="app-name">${label}</div>
    `;
    item.addEventListener('click', async () => await openPath(path));

    appSuggestions.appendChild(item);
    appSuggestions.classList.add('visible');
    resizeWindow();

    return 0; // selectedIndex
}

export function renderSuggestions(apps, appSuggestions, selectedIndex, launchApp, resizeWindow) {
    console.log('Rendering suggestions:', apps);
    appSuggestions.innerHTML = '';
    appSuggestions.scrollTop = 0;

    apps.forEach((app, index) => {
        const item = document.createElement('div');
        item.className = 'app-suggestion-item';
        if (index === selectedIndex) {
            item.classList.add('selected');
        }

        let iconHtml;
        if (app.icon_base64) {
            // Support both raw base64 (PNG) and full data URIs (SVG)
            const src = app.icon_base64.startsWith('data:')
                ? app.icon_base64
                : `data:image/png;base64,${app.icon_base64}`;
            iconHtml = `<img src="${src}" class="app-icon-img" onerror="this.style.display='none'; this.nextElementSibling.style.display='flex';" />
                        <div class="app-icon" style="display:none;">${app.emoji_icon || app.name.charAt(0).toUpperCase()}</div>`;
        } else if (app.emoji_icon) {
            iconHtml = `<div class="app-icon">${app.emoji_icon}</div>`;
        } else {
            const firstLetter = app.name.charAt(0).toUpperCase();
            iconHtml = `<div class="app-icon">${firstLetter}</div>`;
        }

        item.innerHTML = `
            ${iconHtml}
            <div class="app-name">${app.name}</div>
        `;

        item.addEventListener('click', async () => await launchApp(app.name));
        appSuggestions.appendChild(item);
    });

    appSuggestions.classList.add('visible');
    resizeWindow();
}

export function updateSelection(appSuggestions, selectedIndex) {
    const items = appSuggestions.querySelectorAll('.app-suggestion-item');
    items.forEach((item, index) => {
        if (index === selectedIndex) {
            item.classList.add('selected');
            // Scroll within the suggestions container only — don't use scrollIntoView
            // which can scroll parent containers and push the input off screen.
            const itemTop = item.offsetTop - appSuggestions.offsetTop;
            const itemBottom = itemTop + item.offsetHeight;
            if (itemTop < appSuggestions.scrollTop) {
                appSuggestions.scrollTop = itemTop;
            } else if (itemBottom > appSuggestions.scrollTop + appSuggestions.clientHeight) {
                appSuggestions.scrollTop = itemBottom - appSuggestions.clientHeight;
            }
        } else {
            item.classList.remove('selected');
        }
    });
}

export function appendSendHint(container) {
    // Remove existing hint if any
    const existing = container.querySelector('.suggestions-hint');
    if (existing) existing.remove();

    const hint = document.createElement('div');
    hint.className = 'suggestions-hint';
    hint.innerHTML = tHtml('floating.suggestions.shortcut.enter_to_send_html', {
        keys: platformKeyLabel('Ctrl+Enter'),
    });
    container.appendChild(hint);
}

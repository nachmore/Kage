// App suggestions and search functionality

export function renderShortcutSuggestion(shortcut, args, appSuggestions, currentMatches, executeShortcut, resizeWindow) {
    appSuggestions.innerHTML = '';
    currentMatches.length = 0;
    currentMatches.push({ type: 'shortcut', shortcut, args });
    
    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';
    
    item.innerHTML = `
        <div class="app-icon">⚡</div>
        <div class="app-name">${shortcut.name}</div>
    `;
    item.addEventListener('click', async () => await executeShortcut());
    
    appSuggestions.appendChild(item);
    appSuggestions.classList.add('visible');
    setTimeout(() => resizeWindow(), 10);
    
    return 0; // selectedIndex
}

export function renderShortcutSuggestions(matches, appSuggestions, selectedIndex, executeShortcut, resizeWindow) {
    console.log('Rendering multiple shortcut suggestions:', matches);
    appSuggestions.innerHTML = '';
    
    matches.forEach((match, index) => {
        const item = document.createElement('div');
        item.className = 'app-suggestion-item';
        if (index === selectedIndex) {
            item.classList.add('selected');
        }
        
        const actionType = match.shortcut.action_type || 'run_program';
        const actionIcon = actionType === 'open_url' ? '🌐' : '▶️';
        
        item.innerHTML = `
            <div class="app-icon">⚡</div>
            <div class="app-info">
                <div class="app-name">${match.shortcut.name}</div>
                <div class="app-description">${actionIcon} ${match.shortcut.shortcut}</div>
            </div>
        `;
        
        item.addEventListener('click', async () => await executeShortcut(match));
        appSuggestions.appendChild(item);
    });
    
    appSuggestions.classList.add('visible');
    setTimeout(() => resizeWindow(), 10);
}

export function renderUrlSuggestion(url, appSuggestions, currentMatches, openUrl, resizeWindow) {
    appSuggestions.innerHTML = '';
    currentMatches.length = 0;
    currentMatches.push({ type: 'url', value: url });
    
    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';
    item.innerHTML = `
        <div class="app-icon">🌐</div>
        <div class="app-name">Open in browser...</div>
    `;
    item.addEventListener('click', async () => await openUrl(url));
    
    appSuggestions.appendChild(item);
    appSuggestions.classList.add('visible');
    setTimeout(() => resizeWindow(), 10);
    
    return 0; // selectedIndex
}

export function renderPathSuggestion(type, path, appSuggestions, currentMatches, openPath, resizeWindow) {
    appSuggestions.innerHTML = '';
    currentMatches.length = 0;
    currentMatches.push({ type: 'path', value: path, pathType: type });
    
    const item = document.createElement('div');
    item.className = 'app-suggestion-item selected';
    
    const icon = type === 'file' ? '📄' : '📁';
    const label = type === 'file' ? 'Open File' : 'Open Folder';
    
    item.innerHTML = `
        <div class="app-icon">${icon}</div>
        <div class="app-name">${label}: ${path}</div>
    `;
    item.addEventListener('click', async () => await openPath(path));
    
    appSuggestions.appendChild(item);
    appSuggestions.classList.add('visible');
    setTimeout(() => resizeWindow(), 10);
    
    return 0; // selectedIndex
}

export function renderSuggestions(apps, appSuggestions, selectedIndex, launchApp, resizeWindow) {
    console.log('Rendering suggestions:', apps);
    appSuggestions.innerHTML = '';
    
    apps.forEach((app, index) => {
        const item = document.createElement('div');
        item.className = 'app-suggestion-item';
        if (index === selectedIndex) {
            item.classList.add('selected');
        }
        
        let iconHtml;
        if (app.icon_base64) {
            iconHtml = `<img src="data:image/png;base64,${app.icon_base64}" class="app-icon-img" onerror="this.style.display='none'; this.nextElementSibling.style.display='flex';" />
                        <div class="app-icon" style="display:none;">${app.name.charAt(0).toUpperCase()}</div>`;
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
    setTimeout(() => resizeWindow(), 10);
}

export function updateSelection(appSuggestions, selectedIndex) {
    const items = appSuggestions.querySelectorAll('.app-suggestion-item');
    items.forEach((item, index) => {
        if (index === selectedIndex) {
            item.classList.add('selected');
            item.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        } else {
            item.classList.remove('selected');
        }
    });
}

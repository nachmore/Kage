/**
 * About Settings Module
 * Shows version, author, copyright info and links to welcome screen
 */
class AboutSettingsModule extends SettingsModule {
    constructor() {
        super('about', 'About Kage', 'ℹ️');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <div class="about-card">
                    <div class="about-logo-row">
                        <div class="about-logo" id="aboutMascot"></div>
                        <div>
                            <div class="about-app-name">Kage</div>
                            <div class="about-version" id="aboutVersion">loading...</div>
                            <div class="about-homepage" id="aboutHomepage"></div>
                        </div>
                    </div>
                    <div class="about-description" id="aboutDescription"></div>
                    <div class="about-info" id="aboutInfo">
                        <div class="about-row"><span class="about-label">Loading...</span></div>
                    </div>
                    <div class="about-actions">
                        <button class="setting-button" id="showWelcomeBtn">Show Welcome Screen</button>
                    </div>
                </div>
            </div>
        `;
    }

    async initialize() {
        // Render mascot
        const mascotEl = document.getElementById('aboutMascot');
        if (mascotEl) {
            const { createMascot } = await import('../shared/mascot.js');
            const owl = await createMascot({
                size: 72,
                outline: { color: 'var(--kage-mascot-outline, #7138CC)', radius: 1.5 },
            });
            // White body on dark, dark body on light — override the CSS variable on this instance
            owl.style.setProperty('--kage-mascot-body', 'var(--kage-text-bright, #ffffff)');
            mascotEl.appendChild(owl);
        }

        const btn = document.getElementById('showWelcomeBtn');
        if (btn) {
            btn.addEventListener('click', async () => {
                try {
                    await window.__TAURI__.core.invoke('open_welcome_window');
                } catch (e) {
                    console.error('Failed to open welcome window:', e);
                }
            });
        }

        // Load app info from Cargo.toml metadata
        try {
            const info = await window.__TAURI__.core.invoke('get_app_info');
            document.getElementById('aboutVersion').textContent = 'v' + info.version;

            // Homepage link under title
            const hpEl = document.getElementById('aboutHomepage');
            if (hpEl && info.homepage) {
                hpEl.innerHTML = '<a href="' + info.homepage + '" target="_blank">' + info.homepage + '</a>';
            }

            // Description as standalone text
            const descEl = document.getElementById('aboutDescription');
            if (descEl && info.description) {
                descEl.textContent = info.description;
            }

            const infoEl = document.getElementById('aboutInfo');
            if (infoEl) {
                const rows = [];
                if (info.authors) rows.push(this.infoRow('Author', info.authors));
                if (info.repository && info.repository !== 'TBD') rows.push(this.infoRow('Repository', '<a href="' + info.repository + '" target="_blank">' + info.repository.replace('https://', '') + '</a>'));
                if (info.license) rows.push(this.infoRow('License', info.license));
                rows.push(this.infoRow('Copyright', '© 2025 ' + (info.authors || 'Kage Team')));
                infoEl.innerHTML = rows.join('');
            }
        } catch (e) {
            console.log('Failed to load app info:', e);
        }
    }

    infoRow(label, value) {
        return '<div class="about-row"><span class="about-label">' + label + '</span><span>' + value + '</span></div>';
    }

    load(config) {}
    save(config) {}
    validate() { return { valid: true }; }
    destroy() {}
}

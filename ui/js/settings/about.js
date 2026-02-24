/**
 * About Settings Module
 * Shows version, author, copyright info and links to welcome screen
 */
class AboutSettingsModule extends SettingsModule {
    constructor() {
        super('about', 'About Kiro Assistant', 'ℹ️');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                <div class="about-card">
                    <div class="about-logo-row">
                        <img src="../assets/kiro-assistant-icon.png" class="about-logo" alt="Kiro">
                        <div>
                            <div class="about-app-name">Kiro Assistant</div>
                            <div class="about-version" id="aboutVersion">v0.2.0</div>
                        </div>
                    </div>
                    <div class="about-info">
                        <div class="about-row"><span class="about-label">Author</span><span>Kiro Team</span></div>
                        <div class="about-row"><span class="about-label">Website</span><a href="https://github.com/nicholasgasior/kiro-assistant" target="_blank">github.com/nicholasgasior/kiro-assistant</a></div>
                        <div class="about-row"><span class="about-label">License</span><span>MIT</span></div>
                        <div class="about-row"><span class="about-label">Copyright</span><span>© 2025 Kiro Team</span></div>
                    </div>
                    <div class="about-actions">
                        <button class="setting-button" id="showWelcomeBtn">Show Welcome Screen</button>
                    </div>
                </div>
            </div>
        `;
    }

    async initialize() {
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
    }

    load(config) {}
    save(config) {}
    validate() { return { valid: true }; }
    destroy() {}
}

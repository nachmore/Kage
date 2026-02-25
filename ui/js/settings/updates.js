/**
 * Updates Settings Module
 * Auto-update configuration and changelog display
 */
class UpdatesSettingsModule extends SettingsModule {
    constructor() {
        super('updates', 'Updates', '🔄');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                <div class="setting-row">
                    <div class="setting-label">Version</div>
                    <div class="setting-description">
                        Current: <strong id="updateCurrentVersion">loading...</strong>
                        &nbsp;&middot;&nbsp;
                        Latest: <span id="updateLatestVersion">
                            <button class="setting-button" id="checkUpdateBtn">Check Now</button>
                        </span>
                    </div>
                </div>

                ${this.createCheckboxRow(
                    'Automatically Check for Updates',
                    'Check for new versions once per day in the background.',
                    'updateAutoCheck',
                    false
                )}

                ${this.createCheckboxRow(
                    'Silent Updates',
                    'Download and install updates automatically when idle (no floating window activity for 5 minutes).',
                    'updateSilentUpdate',
                    false
                )}

                ${this.createCheckboxRow(
                    'Show Changelog After Update',
                    'Open the Updates page after an update is installed.',
                    'updateShowChangelog',
                    true
                )}

                <div class="setting-row">
                    <div class="setting-label">Changelog</div>
                    <div class="setting-description">Release notes and version history.</div>
                    <div id="changelogContainer" class="changelog-box">
                        <em>Loading changelog...</em>
                    </div>
                </div>
            </div>
        `;
    }

    async initialize() {
        const btn = document.getElementById('checkUpdateBtn');
        if (btn) btn.addEventListener('click', () => this.checkForUpdate());

        try {
            const info = await window.__TAURI__.core.invoke('get_app_info');
            const el = document.getElementById('updateCurrentVersion');
            if (el) el.textContent = 'v' + info.version;
        } catch (e) { /* ignore */ }

        this.loadChangelog();
    }

    async checkForUpdate() {
        const latestEl = document.getElementById('updateLatestVersion');
        if (!latestEl) return;
        latestEl.innerHTML = '<em>Checking...</em>';

        try {
            const result = await window.__TAURI__.core.invoke('check_for_update');
            if (result.available_version) {
                latestEl.innerHTML =
                    'v' + escapeHtml(result.available_version) +
                    ' <span style="color:var(--kiro-accent);font-weight:600;">(update available)</span>' +
                    ' <button class="setting-button" id="installUpdateBtn">Install Now</button>';
                const installBtn = document.getElementById('installUpdateBtn');
                if (installBtn) installBtn.addEventListener('click', () => this.installUpdate());
            } else {
                latestEl.innerHTML =
                    'v' + escapeHtml(result.current_version) + ' (up to date)' +
                    ' <button class="setting-button" id="checkUpdateBtn">Check Again</button>';
                const btn = document.getElementById('checkUpdateBtn');
                if (btn) btn.addEventListener('click', () => this.checkForUpdate());
            }
        } catch (e) {
            latestEl.innerHTML =
                '<em>Check failed</em>' +
                ' <button class="setting-button" id="checkUpdateBtn">Retry</button>';
            const btn = document.getElementById('checkUpdateBtn');
            if (btn) btn.addEventListener('click', () => this.checkForUpdate());
        }
    }

    async installUpdate() {
        const latestEl = document.getElementById('updateLatestVersion');
        if (latestEl) latestEl.innerHTML = '<em>Downloading and installing...</em>';
        try {
            await window.__TAURI__.core.invoke('download_and_install_update');
        } catch (e) {
            if (latestEl) latestEl.innerHTML = '<em>Install failed: ' + escapeHtml(String(e)) + '</em>';
        }
    }

    async loadChangelog() {
        const container = document.getElementById('changelogContainer');
        if (!container) return;
        try {
            let markdown = await window.__TAURI__.core.invoke('fetch_changelog');
            // Escape all HTML tags before parsing. The changelog is markdown, not
            // HTML — this prevents injected <style>/<script> from corrupting the
            // page. Markdown headings, lists, bold, code, and [text](url) links
            // don't use angle brackets, so nothing is lost.
            markdown = markdown.replace(/</g, '&lt;').replace(/>/g, '&gt;');
            if (window.marked) {
                marked.setOptions({ breaks: true, gfm: true });
                container.innerHTML = marked.parse(markdown);
            } else {
                container.textContent = markdown;
            }
        } catch (e) {
            container.innerHTML = '<em>Failed to load changelog.</em>';
        }
    }

    load(config) {
        const u = config.updates || {};
        const autoCheck = document.getElementById('updateAutoCheck');
        const silentUpdate = document.getElementById('updateSilentUpdate');
        const showChangelog = document.getElementById('updateShowChangelog');
        if (autoCheck) autoCheck.checked = u.auto_check || false;
        if (silentUpdate) silentUpdate.checked = u.silent_update || false;
        if (showChangelog) showChangelog.checked = u.show_changelog_after_update !== false;
    }

    save(config) {
        if (!config.updates) config.updates = {};
        config.updates.auto_check = document.getElementById('updateAutoCheck')?.checked || false;
        config.updates.silent_update = document.getElementById('updateSilentUpdate')?.checked || false;
        config.updates.show_changelog_after_update = document.getElementById('updateShowChangelog')?.checked !== false;
    }

    validate() { return { valid: true }; }
    destroy() {}
}

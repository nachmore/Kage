/**
 * Developer Tools Settings Module — extension version.
 * Reads/writes config from config.extensions['dev-tools'].
 */
class DevToolsExtSettingsModule extends SettingsModule {
    constructor() {
        super('devtools', 'Developer Tools', '🛠️');
        this.description = 'UUID generation, base64 encode/decode, hashing, epoch conversion, JSON formatting.';
    }

    renderContent() {
        return `
            <div class="setting-row">
                <div class="setting-label">Individual tools</div>
                <div class="setting-description">Toggle specific developer tools on or off.</div>
            </div>

            ${this.createCheckboxRow('UUID generator', 'Type "uuid" to generate a random UUID v4.', 'devToolUuid', true)}
            ${this.createCheckboxRow('Base64 encode/decode', 'Type "base64 text" to encode or "b64d encoded" to decode.', 'devToolBase64', true)}
            ${this.createCheckboxRow('Hash calculator', 'Type "md5 text", "sha1 text", "sha256 text", or "sha512 text".', 'devToolHash', true)}
            ${this.createCheckboxRow('Epoch/date converter', 'Type a Unix timestamp to see the date, or "now" for the current epoch.', 'devToolEpoch', true)}
            ${this.createCheckboxRow('JSON formatter', 'Paste minified JSON to see it pretty-printed.', 'devToolJson', true)}
        `;
    }

    render() { return this.renderContent(); }

    load(config) {
        const dt = (config.extensions && config.extensions['dev-tools']) || {};
        const setChecked = (id, val) => { const el = document.getElementById(id); if (el) el.checked = val !== false; };
        setChecked('devToolUuid', dt.uuid);
        setChecked('devToolBase64', dt.base64);
        setChecked('devToolHash', dt.hash);
        setChecked('devToolEpoch', dt.epoch);
        setChecked('devToolJson', dt.json_format);
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['dev-tools'] = {
            uuid: document.getElementById('devToolUuid')?.checked ?? true,
            base64: document.getElementById('devToolBase64')?.checked ?? true,
            hash: document.getElementById('devToolHash')?.checked ?? true,
            epoch: document.getElementById('devToolEpoch')?.checked ?? true,
            json_format: document.getElementById('devToolJson')?.checked ?? true,
        };
    }
}
window.DevToolsExtSettingsModule = DevToolsExtSettingsModule;

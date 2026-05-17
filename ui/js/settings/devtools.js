import { SettingsModule } from './base.js';
/**
 * Developer Tools Settings Module
 */
export class DevToolsSettingsModule extends SettingsModule {
    constructor() {
        super('devtools', 'Developer Tools', '🛠️');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>

                ${this.createCheckboxRow(
                    'Enable developer tools',
                    'Detect developer utility commands in the Launcher (UUID, base64, hashing, etc.).',
                    'devToolsEnabled',
                    true
                )}

                <div class="setting-row" style="margin-top: 16px;">
                    <div class="setting-label">Individual tools</div>
                    <div class="setting-description">Toggle specific developer tools on or off.</div>
                </div>

                ${this.createCheckboxRow(
                    'UUID generator',
                    'Type "uuid" to generate a random UUID v4.',
                    'devToolUuid',
                    true
                )}

                ${this.createCheckboxRow(
                    'Base64 encode/decode',
                    'Type "base64 text" to encode or "b64d encoded" to decode.',
                    'devToolBase64',
                    true
                )}

                ${this.createCheckboxRow(
                    'Hash calculator',
                    'Type "md5 text", "sha1 text", "sha256 text", or "sha512 text".',
                    'devToolHash',
                    true
                )}

                ${this.createCheckboxRow(
                    'Epoch/date converter',
                    'Type a Unix timestamp to see the date, or "now" for the current epoch.',
                    'devToolEpoch',
                    true
                )}

                ${this.createCheckboxRow(
                    'JSON formatter',
                    'Paste minified JSON to see it pretty-printed.',
                    'devToolJson',
                    true
                )}
            </div>
        `;
    }

    load(config) {
        const dt = config.dev_tools || {};
        const setChecked = (id, val) => {
            const el = document.getElementById(id);
            if (el) el.checked = val !== false;
        };
        setChecked('devToolsEnabled', dt.enabled);
        setChecked('devToolUuid', dt.uuid);
        setChecked('devToolBase64', dt.base64);
        setChecked('devToolHash', dt.hash);
        setChecked('devToolEpoch', dt.epoch);
        setChecked('devToolJson', dt.json_format);
    }

    save(config) {
        config.dev_tools = {
            enabled: document.getElementById('devToolsEnabled')?.checked ?? true,
            uuid: document.getElementById('devToolUuid')?.checked ?? true,
            base64: document.getElementById('devToolBase64')?.checked ?? true,
            hash: document.getElementById('devToolHash')?.checked ?? true,
            epoch: document.getElementById('devToolEpoch')?.checked ?? true,
            json_format: document.getElementById('devToolJson')?.checked ?? true,
        };
    }
}

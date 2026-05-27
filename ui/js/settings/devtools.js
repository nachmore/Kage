import { SettingsModule } from './base.js';
/**
 * Developer Tools Settings Module
 */
export class DevToolsSettingsModule extends SettingsModule {
    constructor() {
        super('devtools', 'Developer Tools', '🛠️');
        this.bindFields([
            { id: 'devToolsEnabled', path: 'dev_tools.enabled', kind: 'checkbox', default: true },
            { id: 'devToolUuid', path: 'dev_tools.uuid', kind: 'checkbox', default: true },
            { id: 'devToolBase64', path: 'dev_tools.base64', kind: 'checkbox', default: true },
            { id: 'devToolHash', path: 'dev_tools.hash', kind: 'checkbox', default: true },
            { id: 'devToolEpoch', path: 'dev_tools.epoch', kind: 'checkbox', default: true },
            {
                id: 'devToolJson',
                path: 'dev_tools.json_format',
                kind: 'checkbox',
                default: true,
            },
        ]);
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
        this.loadFields(config);
    }

    save(config) {
        this.saveFields(config);
    }
}

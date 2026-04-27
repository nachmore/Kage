/**
 * Developer Tools settings provider (sandboxed).
 */
export default class DevToolsSettingsProvider {
    initialize(context) { this.config = context.config || {}; }
    onConfigUpdate(config) { this.config = config || {}; }

    getSettings() {
        return {
            description: 'UUID generation, base64 encode/decode, hashing, epoch conversion, JSON formatting.',
            sections: [
                {
                    label: 'Individual tools',
                    controls: [
                        { type: 'checkbox', id: 'uuid',        label: 'UUID generator',      description: 'Type "uuid" to generate a random UUID v4.', default: true },
                        { type: 'checkbox', id: 'base64',      label: 'Base64 encode/decode', description: 'Type "base64 text" to encode or "b64d encoded" to decode.', default: true },
                        { type: 'checkbox', id: 'hash',        label: 'Hash calculator',     description: 'Type "md5 text", "sha1 text", "sha256 text", or "sha512 text".', default: true },
                        { type: 'checkbox', id: 'epoch',       label: 'Epoch/date converter', description: 'Type a Unix timestamp to see the date, or "now" for the current epoch.', default: true },
                        { type: 'checkbox', id: 'json_format', label: 'JSON formatter',      description: 'Paste minified JSON to see it pretty-printed.', default: true },
                    ],
                },
            ],
        };
    }
}

/**
 * Dictionary settings provider (sandboxed).
 */

const LANGUAGES = [
    { code: 'auto', name: 'Auto-detect' },
    { code: 'en', name: 'English' },
    { code: 'la', name: 'Latin' },
    { code: 'es', name: 'Spanish' },
    { code: 'it', name: 'Italian' },
    { code: 'ru', name: 'Russian' },
    { code: 'pt', name: 'Portuguese' },
    { code: 'fr', name: 'French' },
    { code: 'de', name: 'German' },
    { code: 'sv', name: 'Swedish' },
    { code: 'fi', name: 'Finnish' },
    { code: 'zh', name: 'Chinese' },
    { code: 'pl', name: 'Polish' },
    { code: 'nl', name: 'Dutch' },
    { code: 'ro', name: 'Romanian' },
    { code: 'ja', name: 'Japanese' },
    { code: 'el', name: 'Greek' },
    { code: 'hu', name: 'Hungarian' },
    { code: 'cs', name: 'Czech' },
    { code: 'uk', name: 'Ukrainian' },
    { code: 'da', name: 'Danish' },
    { code: 'bg', name: 'Bulgarian' },
    { code: 'ko', name: 'Korean' },
    { code: 'tr', name: 'Turkish' },
    { code: 'vi', name: 'Vietnamese' },
    { code: 'hi', name: 'Hindi' },
    { code: 'ar', name: 'Arabic' },
    { code: 'th', name: 'Thai' },
    { code: 'fa', name: 'Persian' },
    { code: 'he', name: 'Hebrew' },
    { code: 'id', name: 'Indonesian' },
];

export default class DictionarySettingsProvider {
    initialize(context) { this.config = context.config || {}; }
    onConfigUpdate(config) { this.config = config || {}; }

    getSettings() {
        return {
            description: 'Look up word definitions, spelling corrections, and pronunciation. Powered by FreeDictionaryAPI.com (Wiktionary data).',
            sections: [
                {
                    controls: [
                        {
                            type: 'text',
                            id: 'trigger',
                            label: 'Trigger Keyword',
                            description: 'Type this keyword followed by a space to activate dictionary lookup (e.g. "dict hello"). Leave empty to look up any typed word.',
                            default: 'dict',
                            placeholder: 'dict',
                            maxWidth: 100,
                        },
                        {
                            type: 'select',
                            id: 'language',
                            label: 'Language',
                            description: 'Dictionary language for lookups. Auto-detect uses tinyld to identify the language. Supports 250+ languages via FreeDictionaryAPI.com.',
                            default: 'auto',
                            options: LANGUAGES.map(l => ({ value: l.code, label: l.name })),
                        },
                        { type: 'checkbox', id: 'show_pronunciation', label: 'Show Pronunciation', description: 'Display IPA pronunciation when available', default: true },
                        { type: 'checkbox', id: 'show_examples',      label: 'Show Examples',       description: 'Display usage examples when available', default: true },
                        { type: 'checkbox', id: 'show_synonyms',      label: 'Show Synonyms',       description: 'Display synonyms when available', default: true },
                        {
                            type: 'info',
                            html: 'Data sourced from <a href="https://en.wiktionary.org/" target="_blank">Wiktionary</a> '
                                + 'via <a href="https://freedictionaryapi.com/" target="_blank">FreeDictionaryAPI.com</a> '
                                + 'under <a href="https://creativecommons.org/licenses/by-sa/4.0/" target="_blank">CC BY-SA 4.0</a>. '
                                + 'Spelling suggestions by <a href="https://www.datamuse.com/" target="_blank">Datamuse</a>.',
                        },
                    ],
                },
            ],
        };
    }
}

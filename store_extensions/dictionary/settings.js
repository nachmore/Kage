/**
 * Dictionary Settings Module — extension settings page.
 * Reads/writes config from config.extensions['dictionary'].
 */

// Top languages from FreeDictionaryAPI.com (sorted by word count)
const LANGUAGES = [
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

class DictionaryExtSettingsModule extends SettingsModule {
    constructor() {
        super('dictionary', 'Dictionary', '📖');
        this.description = 'Look up word definitions, spelling corrections, and pronunciation. Powered by FreeDictionaryAPI.com (Wiktionary data).';
    }

    renderContent() {
        const options = LANGUAGES.map(l =>
            `<option value="${l.code}">${l.name}</option>`
        ).join('');

        return `
            ${this.createControlRow(
                'Trigger Keyword',
                'Type this keyword followed by a space to activate dictionary lookup (e.g. "dict hello"). Leave empty to look up any typed word.',
                '<input type="text" class="setting-input" id="dictTrigger" placeholder="dict" value="dict" style="width: 100px;">'
            )}
            ${this.createControlRow(
                'Language',
                'Dictionary language for lookups. Supports 250+ languages via FreeDictionaryAPI.com.',
                `<select class="setting-input" id="dictLanguage">${options}</select>`
            )}
            ${this.createCheckboxRow('Show Pronunciation', 'Display IPA pronunciation when available', 'dictShowPronunciation', true)}
            ${this.createCheckboxRow('Show Examples', 'Display usage examples when available', 'dictShowExamples', true)}
            ${this.createCheckboxRow('Show Synonyms', 'Display synonyms when available', 'dictShowSynonyms', true)}
            <div class="setting-row" style="margin-top: 12px; opacity: 0.7; font-size: 12px;">
                Data sourced from <a href="https://en.wiktionary.org/" target="_blank" style="color: var(--kiro-accent);">Wiktionary</a>
                via <a href="https://freedictionaryapi.com/" target="_blank" style="color: var(--kiro-accent);">FreeDictionaryAPI.com</a>
                under <a href="https://creativecommons.org/licenses/by-sa/4.0/" target="_blank" style="color: var(--kiro-accent);">CC BY-SA 4.0</a>.
                Spelling suggestions by <a href="https://www.datamuse.com/" target="_blank" style="color: var(--kiro-accent);">Datamuse</a>.
            </div>
        `;
    }

    render() { return this.renderContent(); }

    load(config) {
        const ext = (config.extensions && config.extensions['dictionary']) || {};
        const trigger = document.getElementById('dictTrigger');
        const lang = document.getElementById('dictLanguage');
        const pron = document.getElementById('dictShowPronunciation');
        const examples = document.getElementById('dictShowExamples');
        const syn = document.getElementById('dictShowSynonyms');
        if (trigger) trigger.value = ext.trigger ?? 'dict';
        if (lang) lang.value = ext.language || 'en';
        if (pron) pron.checked = ext.show_pronunciation !== false;
        if (examples) examples.checked = ext.show_examples !== false;
        if (syn) syn.checked = ext.show_synonyms !== false;
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['dictionary'] = {
            trigger: document.getElementById('dictTrigger')?.value ?? 'dict',
            language: document.getElementById('dictLanguage')?.value || 'en',
            show_pronunciation: document.getElementById('dictShowPronunciation')?.checked ?? true,
            show_examples: document.getElementById('dictShowExamples')?.checked ?? true,
            show_synonyms: document.getElementById('dictShowSynonyms')?.checked ?? true,
        };
    }

    validate() { return { valid: true }; }
}

window.DictionaryExtSettingsModule = DictionaryExtSettingsModule;

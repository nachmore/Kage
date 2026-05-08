/**
 * Hotkey & Shortcuts Settings Module - uses shared HotkeyPicker component
 */
class HotkeySettingsModule extends SettingsModule {
    constructor() {
        super('hotkey', 'Hotkey & Shortcuts', '\u{1F3B9}');
        this._picker = null;
        this._cbPicker = null;
        this._iaPicker = null;
        this._voicePicker = null;
    }
    render() {
        return '<div class="settings-section" id="' + this.id + '-section">'
            + '<h2 class="settings-section-header">' + this.icon + ' ' + this.title + '</h2>'
            + this.createControlRow('Global Hotkey', 'The shortcut to summon Kage from anywhere.', '<div id="settingsHotkeyPicker"></div>')
            + this.createControlRow('Voice Input Hotkey', 'summon Kage with the microphone already listening. Leave empty to disable.', '<div id="settingsVoiceHotkeyPicker"></div>')
            + this.createControlRow('Clipboard History Hotkey', 'Open clipboard history directly. Leave empty to disable.', '<div id="settingsClipboardHotkeyPicker"></div>')
            + this.createControlRow('Inline Assist Hotkey', 'Trigger inline AI assist on selected text. Leave empty to disable.', '<div id="settingsInlineAssistHotkeyPicker"></div>')
            + '<div class="setting-section-label">Keyboard Shortcuts</div>'
            + '<div class="shortcuts-reference">'
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Ctrl+N'), 'New session', 'Chat window')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Ctrl+W'), 'Close / hide window', 'All windows')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Ctrl+,'), 'Open settings', 'Launcher & Chat')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Ctrl+E'), 'Expand to full chat', 'Launcher')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Ctrl+L'), 'Clear / reset', 'Launcher')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Ctrl+Shift+C'), 'Copy last response', 'Launcher & Chat')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Ctrl+Enter'), 'Send to agent (bypass suggestions)', 'Launcher')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Escape'), 'Stop generating / Hide window', 'Launcher')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Enter'), 'Send message', 'All')
            + this.shortcutRow(window.kagePlatform.platformKeyLabel('Shift+Enter'), 'New line', 'All')
            + this.shortcutRow('>cb', 'Clipboard history', 'Launcher')
            + '</div></div>';
    }
    async initialize() {
        const { HotkeyPicker } = await import('../shared/hotkey-picker.js');
        const invoke = window.__TAURI__.core.invoke;
        const container = document.getElementById('settingsHotkeyPicker');
        if (!container) return;
        this._picker = new HotkeyPicker(container, invoke, { modifiers: ['Alt'], key: 'Space' }, 'main');
        this._picker.onChange(async (hk) => {
            try {
                const config = await invoke('get_config');
                config.hotkey = hk;
                await invoke('save_config', { config });
            } catch (e) { console.error('Failed to save hotkey:', e); }
        });

        // Clipboard history hotkey picker
        const cbContainer = document.getElementById('settingsClipboardHotkeyPicker');
        if (cbContainer) {
            this._cbPicker = new HotkeyPicker(cbContainer, invoke, { modifiers: [], key: '' }, 'clipboard');
            this._cbPicker.onChange(async (hk) => {
                try {
                    const config = await invoke('get_config');
                    config.clipboard_hotkey = (hk.key) ? hk : null;
                    await invoke('save_config', { config });
                } catch (e) { console.error('Failed to save clipboard hotkey:', e); }
            });
        }

        // Inline assist hotkey picker
        const iaContainer = document.getElementById('settingsInlineAssistHotkeyPicker');
        if (iaContainer) {
            this._iaPicker = new HotkeyPicker(iaContainer, invoke, { modifiers: [], key: '' }, 'inline-assist');
            this._iaPicker.onChange(async (hk) => {
                try {
                    const config = await invoke('get_config');
                    config.inline_assist_hotkey = (hk.key) ? hk : null;
                    await invoke('save_config', { config });
                } catch (e) { console.error('Failed to save inline assist hotkey:', e); }
            });
        }

        // Voice input hotkey picker
        const voiceContainer = document.getElementById('settingsVoiceHotkeyPicker');
        if (voiceContainer) {
            this._voicePicker = new HotkeyPicker(voiceContainer, invoke, { modifiers: [], key: '' }, 'voice');
            this._voicePicker.onChange(async (hk) => {
                try {
                    const config = await invoke('get_config');
                    config.voice_hotkey = (hk.key) ? hk : null;
                    await invoke('save_config', { config });
                } catch (e) { console.error('Failed to save voice hotkey:', e); }
            });
        }

        document.querySelectorAll('.shortcut-ref-keys[data-keys]').forEach(el => {
            this.renderKeycaps(el, el.dataset.keys);
        });
    }
    load(config) {
        if (config.hotkey && this._picker) this._picker.setHotkey(config.hotkey);
        if (this._cbPicker) {
            if (config.clipboard_hotkey) {
                this._cbPicker.setHotkey(config.clipboard_hotkey);
            } else {
                this._cbPicker.setHotkey({ modifiers: [], key: '' });
            }
        }
        if (this._iaPicker) {
            if (config.inline_assist_hotkey) {
                this._iaPicker.setHotkey(config.inline_assist_hotkey);
            } else {
                // Show the default hotkey
                this._iaPicker.setHotkey({ modifiers: ['Ctrl', 'Shift'], key: 'Space' });
            }
        }
        if (this._voicePicker) {
            if (config.voice_hotkey) {
                this._voicePicker.setHotkey(config.voice_hotkey);
            } else {
                this._voicePicker.setHotkey({ modifiers: [], key: '' });
            }
        }
    }
    save(config) {
        // Hotkey is saved immediately on capture via try_register_hotkey,
        // but we need to ensure the field exists for the save_config call
        if (!config.hotkey && this._picker) {
            config.hotkey = this._picker.hotkey;
        }
        if (this._cbPicker) {
            const cbHk = this._cbPicker.hotkey;
            config.clipboard_hotkey = (cbHk && cbHk.key) ? cbHk : null;
        }
        if (this._iaPicker) {
            const iaHk = this._iaPicker.hotkey;
            config.inline_assist_hotkey = (iaHk && iaHk.key) ? iaHk : null;
        } else if (!config.inline_assist_hotkey) {
            // Ensure the default is written to config on first save
            config.inline_assist_hotkey = { modifiers: ['Ctrl', 'Shift'], key: 'Space' };
        }
        if (this._voicePicker) {
            const vHk = this._voicePicker.hotkey;
            config.voice_hotkey = (vHk && vHk.key) ? vHk : null;
        }
    }
    renderKeycaps(container, hotkeyStr) {
        if (!container || !hotkeyStr) return;
        const parts = hotkeyStr.split('+').map(s => s.trim()).filter(Boolean);
        container.innerHTML = parts.map((key, i) => {
            const sep = i < parts.length - 1 ? '<span class="keycap-sep">+</span>' : '';
            return '<kbd class="keycap">' + key + '</kbd>' + sep;
        }).join('');
    }
    validate() { return { valid: true }; }
    shortcutRow(keys, description, scope) {
        return '<div class="shortcut-ref-row">'
            + '<span class="shortcut-ref-keys" data-keys="' + keys + '"></span>'
            + '<span class="shortcut-ref-desc">' + description + '</span>'
            + '<span class="shortcut-ref-scope">' + scope + '</span>'
            + '</div>';
    }
    destroy() { this._picker = null; this._cbPicker = null; this._iaPicker = null; this._voicePicker = null; }
}

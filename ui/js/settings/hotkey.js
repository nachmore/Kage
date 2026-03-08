/**
 * Hotkey & Shortcuts Settings Module - uses shared HotkeyPicker component
 */
class HotkeySettingsModule extends SettingsModule {
    constructor() {
        super('hotkey', 'Hotkey & Shortcuts', '\u{1F3B9}');
        this._picker = null;
        this._cbPicker = null;
    }
    render() {
        return '<div class="settings-section" id="' + this.id + '-section">'
            + '<h2 class="settings-section-header">' + this.icon + ' ' + this.title + '</h2>'
            + this.createControlRow('Global Hotkey', 'The shortcut to summon Kiro from anywhere.', '<div id="settingsHotkeyPicker"></div>')
            + this.createControlRow('Clipboard History Hotkey', 'Open clipboard history directly. Leave empty to disable.', '<div id="settingsClipboardHotkeyPicker"></div>')
            + '<div class="setting-row" style="margin-top: 6px;"><div class="setting-label">Keyboard Shortcuts</div><div class="setting-description">Built-in shortcuts available across the application.</div></div>'
            + '<div class="shortcuts-reference">'
            + this.shortcutRow('Ctrl+N', 'New session', 'Chat window')
            + this.shortcutRow('Ctrl+W', 'Close / hide window', 'All windows')
            + this.shortcutRow('Ctrl+,', 'Open settings', 'Floating & Chat')
            + this.shortcutRow('Ctrl+E', 'Expand to full chat', 'Floating')
            + this.shortcutRow('Ctrl+L', 'Clear / reset', 'Floating')
            + this.shortcutRow('Ctrl+Shift+C', 'Copy last response', 'Floating & Chat')
            + this.shortcutRow('Ctrl+Enter', 'Send to agent (bypass suggestions)', 'Floating')
            + this.shortcutRow('Escape', 'Stop generating / Hide window', 'Floating')
            + this.shortcutRow('Enter', 'Send message', 'All')
            + this.shortcutRow('Shift+Enter', 'New line', 'All')
            + this.shortcutRow('>cb', 'Clipboard history', 'Floating')
            + '</div></div>';
    }
    async initialize() {
        const { HotkeyPicker } = await import('../shared/hotkey-picker.js');
        const invoke = window.__TAURI__.core.invoke;
        const container = document.getElementById('settingsHotkeyPicker');
        if (!container) return;
        this._picker = new HotkeyPicker(container, invoke, { modifiers: ['Alt'], key: 'Space' });
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
            this._cbPicker = new HotkeyPicker(cbContainer, invoke, { modifiers: [], key: '' });
            this._cbPicker.onChange(async (hk) => {
                try {
                    const config = await invoke('get_config');
                    config.clipboard_hotkey = (hk.key) ? hk : null;
                    await invoke('save_config', { config });
                } catch (e) { console.error('Failed to save clipboard hotkey:', e); }
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
    destroy() { this._picker = null; this._cbPicker = null; }
}

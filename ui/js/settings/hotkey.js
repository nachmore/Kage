import { SettingsModule } from './base.js';
import { isWindows, platformKeyLabel } from '../shared/shortcuts.js';
import { t } from '../shared/i18n.js';
/**
 * Hotkey & Shortcuts Settings Module - uses shared HotkeyPicker component
 */
export class HotkeySettingsModule extends SettingsModule {
    constructor() {
        super('hotkey', t('settings.hotkey.title'), '\u{1F3B9}');
        this._picker = null;
        this._cbPicker = null;
        this._iaPicker = null;
        this._voicePicker = null;
    }
    render() {
        // Clipboard history is implemented on Windows only today (macOS returns
        // an empty list by design — see src/os/macos/clipboard_history.rs). Hide
        // the hotkey binding and quick-reference row on non-Windows so users
        // don't set a shortcut that would never do anything.
        const showClipboardHistory = isWindows() ?? false;
        const clipboardHotkeyRow = showClipboardHistory
            ? this.createControlRow(
                  t('settings.hotkey.clipboard.label'),
                  t('settings.hotkey.clipboard.description'),
                  '<div id="settingsClipboardHotkeyPicker"></div>'
              )
            : '';
        const clipboardShortcutRef = showClipboardHistory
            ? this.shortcutRow(
                  '>cb',
                  t('settings.hotkey.shortcut.clipboard_history.action'),
                  t('settings.hotkey.scope.launcher')
              )
            : '';

        return (
            '<div class="settings-section" id="' +
            this.id +
            '-section">' +
            '<h2 class="settings-section-header">' +
            this.icon +
            ' ' +
            this.title +
            '</h2>' +
            '<div id="hotkeyRegistrationWarning" class="settings-warning" style="display:none;"></div>' +
            this.createControlRow(
                t('settings.hotkey.global.label'),
                t('settings.hotkey.global.description'),
                '<div id="settingsHotkeyPicker"></div>'
            ) +
            this.createControlRow(
                t('settings.hotkey.voice.label'),
                t('settings.hotkey.voice.description'),
                '<div id="settingsVoiceHotkeyPicker"></div>'
            ) +
            clipboardHotkeyRow +
            this.createControlRow(
                t('settings.hotkey.inline_assist.label'),
                t('settings.hotkey.inline_assist.description'),
                '<div id="settingsInlineAssistHotkeyPicker"></div>'
            ) +
            '<div class="setting-section-label">' +
            t('settings.hotkey.shortcuts.section') +
            '</div>' +
            '<div class="shortcuts-reference">' +
            this.shortcutRow(
                platformKeyLabel('Ctrl+N'),
                t('settings.hotkey.shortcut.new_session.action'),
                t('settings.hotkey.scope.chat_window')
            ) +
            this.shortcutRow(
                platformKeyLabel('Ctrl+W'),
                t('settings.hotkey.shortcut.close_window.action'),
                t('settings.hotkey.scope.all_windows')
            ) +
            this.shortcutRow(
                platformKeyLabel('Ctrl+,'),
                t('settings.hotkey.shortcut.open_settings.action'),
                t('settings.hotkey.scope.launcher_chat')
            ) +
            this.shortcutRow(
                platformKeyLabel('Ctrl+E'),
                t('settings.hotkey.shortcut.expand_chat.action'),
                t('settings.hotkey.scope.launcher')
            ) +
            this.shortcutRow(
                platformKeyLabel('Ctrl+L'),
                t('settings.hotkey.shortcut.clear.action'),
                t('settings.hotkey.scope.launcher')
            ) +
            this.shortcutRow(
                platformKeyLabel('Ctrl+Shift+C'),
                t('settings.hotkey.shortcut.copy_response.action'),
                t('settings.hotkey.scope.launcher_chat')
            ) +
            this.shortcutRow(
                platformKeyLabel('Ctrl+Enter'),
                t('settings.hotkey.shortcut.send_bypass.action'),
                t('settings.hotkey.scope.launcher')
            ) +
            this.shortcutRow(
                platformKeyLabel('Escape'),
                t('settings.hotkey.shortcut.escape.action'),
                t('settings.hotkey.scope.launcher')
            ) +
            this.shortcutRow(
                platformKeyLabel('Enter'),
                t('settings.hotkey.shortcut.send.action'),
                t('settings.hotkey.scope.all')
            ) +
            this.shortcutRow(
                platformKeyLabel('Shift+Enter'),
                t('settings.hotkey.shortcut.newline.action'),
                t('settings.hotkey.scope.all')
            ) +
            clipboardShortcutRef +
            '</div></div>'
        );
    }
    async initialize() {
        const { HotkeyPicker } = await import('../shared/hotkey-picker.js');
        const invoke = window.__TAURI__.core.invoke;
        const container = document.getElementById('settingsHotkeyPicker');
        if (!container) return;
        this._picker = new HotkeyPicker(
            container,
            invoke,
            { modifiers: ['Alt'], key: 'Space' },
            'main'
        );
        this._picker.onChange(async (hk) => {
            try {
                const config = await invoke('get_config');
                config.hotkey = hk;
                await invoke('save_config', { config });
            } catch (e) {
                console.error('Failed to save hotkey:', e);
            }
        });

        // Clipboard history hotkey picker
        const cbContainer = document.getElementById('settingsClipboardHotkeyPicker');
        if (cbContainer) {
            this._cbPicker = new HotkeyPicker(
                cbContainer,
                invoke,
                { modifiers: [], key: '' },
                'clipboard'
            );
            this._cbPicker.onChange(async (hk) => {
                try {
                    const config = await invoke('get_config');
                    config.clipboard_hotkey = hk.key ? hk : null;
                    await invoke('save_config', { config });
                } catch (e) {
                    console.error('Failed to save clipboard hotkey:', e);
                }
            });
        }

        // Inline assist hotkey picker
        const iaContainer = document.getElementById('settingsInlineAssistHotkeyPicker');
        if (iaContainer) {
            this._iaPicker = new HotkeyPicker(
                iaContainer,
                invoke,
                { modifiers: [], key: '' },
                'inline-assist'
            );
            this._iaPicker.onChange(async (hk) => {
                try {
                    const config = await invoke('get_config');
                    config.inline_assist_hotkey = hk.key ? hk : null;
                    await invoke('save_config', { config });
                } catch (e) {
                    console.error('Failed to save inline assist hotkey:', e);
                }
            });
        }

        // Voice input hotkey picker
        const voiceContainer = document.getElementById('settingsVoiceHotkeyPicker');
        if (voiceContainer) {
            this._voicePicker = new HotkeyPicker(
                voiceContainer,
                invoke,
                { modifiers: [], key: '' },
                'voice'
            );
            this._voicePicker.onChange(async (hk) => {
                try {
                    const config = await invoke('get_config');
                    config.voice_hotkey = hk.key ? hk : null;
                    await invoke('save_config', { config });
                } catch (e) {
                    console.error('Failed to save voice hotkey:', e);
                }
            });
        }

        document.querySelectorAll('.shortcut-ref-keys[data-keys]').forEach((el) => {
            this.renderKeycaps(el, el.dataset.keys);
        });

        // Surface hotkey-registration failures (another app owns the combo).
        // The event may have fired at startup before this window existed, so
        // also ask the backend for the last-known failures on open.
        const { listen } = window.__TAURI__.event;
        this._unlistenHotkeyFail = await listen('hotkey_registration_failed', (e) => {
            this._showRegistrationWarning(e.payload);
        });
        try {
            const pending = await invoke('get_hotkey_registration_failures');
            if (pending && pending.length) this._showRegistrationWarning(pending);
        } catch {
            /* command optional; ignore if unavailable */
        }
    }

    _showRegistrationWarning(failures) {
        const el = document.getElementById('hotkeyRegistrationWarning');
        if (!el || !Array.isArray(failures) || failures.length === 0) return;
        const combos = failures.map((f) => f.hotkey).join(', ');
        el.textContent = t('settings.hotkey.registration_failed', { combos });
        el.style.display = 'block';
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
            config.clipboard_hotkey = cbHk?.key ? cbHk : null;
        }
        if (this._iaPicker) {
            const iaHk = this._iaPicker.hotkey;
            config.inline_assist_hotkey = iaHk?.key ? iaHk : null;
        } else if (!config.inline_assist_hotkey) {
            // Ensure the default is written to config on first save
            config.inline_assist_hotkey = { modifiers: ['Ctrl', 'Shift'], key: 'Space' };
        }
        if (this._voicePicker) {
            const vHk = this._voicePicker.hotkey;
            config.voice_hotkey = vHk?.key ? vHk : null;
        }
    }
    renderKeycaps(container, hotkeyStr) {
        if (!container || !hotkeyStr) return;
        const parts = hotkeyStr
            .split('+')
            .map((s) => s.trim())
            .filter(Boolean);
        container.innerHTML = parts
            .map((key, i) => {
                const sep = i < parts.length - 1 ? '<span class="keycap-sep">+</span>' : '';
                return '<kbd class="keycap">' + key + '</kbd>' + sep;
            })
            .join('');
    }
    validate() {
        return { valid: true };
    }
    shortcutRow(keys, description, scope) {
        return (
            '<div class="shortcut-ref-row">' +
            '<span class="shortcut-ref-keys" data-keys="' +
            keys +
            '"></span>' +
            '<span class="shortcut-ref-desc">' +
            description +
            '</span>' +
            '<span class="shortcut-ref-scope">' +
            scope +
            '</span>' +
            '</div>'
        );
    }
    destroy() {
        this._picker = null;
        this._cbPicker = null;
        this._iaPicker = null;
        this._voicePicker = null;
        if (this._unlistenHotkeyFail) {
            this._unlistenHotkeyFail();
            this._unlistenHotkeyFail = null;
        }
    }
}

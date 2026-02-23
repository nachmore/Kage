/**
 * Hotkey Settings Module — custom dropdown with keycap badges
 */
class HotkeySettingsModule extends SettingsModule {
    constructor() {
        super('hotkey', 'Hotkey', '🎹');
        this._selectedValue = 'Alt+Space';
        this._customHotkey = '';
        this._capturing = false;
        this._open = false;
    }

    render() {
        const presets = [
            { value: 'Alt+Space', label: 'default' },
            { value: 'Ctrl+Space' },
            { value: 'Alt+K' },
            { value: 'Ctrl+Shift+K' },
            { value: 'Alt+Shift+Space' },
            { value: 'Ctrl+Shift+Space' },
            { value: 'Alt+J' },
            { value: 'Ctrl+Shift+J' },
        ];

        const items = presets.map(p => {
            const label = p.label ? ' <span class="hotkey-dropdown-label">' + p.label + '</span>' : '';
            return '<div class="hotkey-dropdown-item" data-value="' + p.value + '" onclick="hotkeyModule.selectPreset(this)">'
                + '<span class="hotkey-dropdown-keycaps-inner" data-keys="' + p.value + '"></span>' + label + '</div>';
        }).join('');

        return '<div class="settings-section" id="' + this.id + '-section">'
            + '<h2 class="settings-section-header">' + this.icon + ' ' + this.title + '</h2>'
            + this.createControlRow('Global Hotkey', 'Choose a preset or capture a custom hotkey combination.',
                '<div class="hotkey-dropdown" id="hotkeyDropdown">'
                + '<div class="hotkey-dropdown-selected" id="hotkeySelected" onclick="hotkeyModule.toggleDropdown()">'
                +   '<span class="keycap-preview" id="hotkeySelectedKeycaps"></span>'
                +   '<span class="hotkey-dropdown-arrow">▾</span>'
                + '</div>'
                + '<div class="hotkey-dropdown-menu" id="hotkeyMenu" style="display:none;">'
                +   items
                +   '<div class="hotkey-dropdown-divider"></div>'
                +   '<div class="hotkey-dropdown-item" data-value="custom" onclick="hotkeyModule.selectPreset(this)">'
                +     '<span style="opacity:0.6">Custom…</span>'
                +   '</div>'
                + '</div>'
                + '</div>'
                + '<div id="customHotkeyRow" style="display:none; margin-top: 8px;">'
                +   '<div style="display: flex; gap: 8px; align-items: center;">'
                +     '<div class="keycap-preview" id="customHotkeyDisplay"></div>'
                +     '<button class="setting-button" id="captureBtn" onclick="hotkeyModule.startCapture()">Capture</button>'
                +   '</div>'
                +   '<div class="setting-description" style="margin-top: 4px;">Click Capture, then press your desired key combination.</div>'
                + '</div>'
            )
            + '</div>';
    }

    initialize() {
        window.hotkeyModule = this;
        // Render keycaps inside each dropdown item
        document.querySelectorAll('.hotkey-dropdown-keycaps-inner[data-keys]').forEach(el => {
            this.renderKeycaps(el, el.dataset.keys);
        });
        // Close dropdown on outside click
        document.addEventListener('click', (e) => {
            if (this._open && !document.getElementById('hotkeyDropdown')?.contains(e.target)) {
                this.closeDropdown();
            }
        });
    }

    load(config) {
        if (!config.hotkey) return;
        const hotkeyStr = config.hotkey.modifiers.join('+') + '+' + config.hotkey.key;
        const presetExists = document.querySelector('.hotkey-dropdown-item[data-value="' + hotkeyStr + '"]');
        if (presetExists) {
            this._selectedValue = hotkeyStr;
        } else {
            this._selectedValue = 'custom';
            this._customHotkey = hotkeyStr;
            const customDisplay = document.getElementById('customHotkeyDisplay');
            if (customDisplay) this.renderKeycaps(customDisplay, hotkeyStr);
            document.getElementById('customHotkeyRow').style.display = '';
        }
        this.renderKeycaps(document.getElementById('hotkeySelectedKeycaps'),
            this._selectedValue === 'custom' ? null : this._selectedValue);
        if (this._selectedValue === 'custom') {
            document.getElementById('hotkeySelectedKeycaps').innerHTML = '<span style="opacity:0.5">Custom…</span>';
        }
        this.updateActiveItem();
    }

    save(config) {
        const hotkeyStr = this._selectedValue === 'custom' ? (this._customHotkey || 'Alt+Space') : this._selectedValue;
        const parts = hotkeyStr.split('+').map(s => s.trim()).filter(Boolean);
        config.hotkey = parts.length >= 2
            ? { modifiers: parts.slice(0, -1), key: parts[parts.length - 1] }
            : { modifiers: ['Alt'], key: 'Space' };
    }

    toggleDropdown() {
        this._open ? this.closeDropdown() : this.openDropdown();
    }

    openDropdown() {
        document.getElementById('hotkeyMenu').style.display = '';
        this._open = true;
    }

    closeDropdown() {
        document.getElementById('hotkeyMenu').style.display = 'none';
        this._open = false;
    }

    selectPreset(el) {
        const value = el.dataset.value;
        this._selectedValue = value;
        this.closeDropdown();

        const customRow = document.getElementById('customHotkeyRow');
        if (value === 'custom') {
            customRow.style.display = '';
            document.getElementById('hotkeySelectedKeycaps').innerHTML = '<span style="opacity:0.5">Custom…</span>';
        } else {
            customRow.style.display = 'none';
            this.renderKeycaps(document.getElementById('hotkeySelectedKeycaps'), value);
        }
        this.updateActiveItem();
    }

    updateActiveItem() {
        document.querySelectorAll('.hotkey-dropdown-item').forEach(el => {
            el.classList.toggle('active', el.dataset.value === this._selectedValue);
        });
    }

    renderKeycaps(container, hotkeyStr) {
        if (!container || !hotkeyStr) return;
        const parts = hotkeyStr.split('+').map(s => s.trim()).filter(Boolean);
        container.innerHTML = parts.map((key, i) => {
            const sep = i < parts.length - 1 ? '<span class="keycap-sep">+</span>' : '';
            return '<kbd class="keycap">' + escapeHtml(key) + '</kbd>' + sep;
        }).join('');
    }

    startCapture() {
        if (this._capturing) { this.stopCapture(); return; }
        this._capturing = true;
        const btn = document.getElementById('captureBtn');
        const display = document.getElementById('customHotkeyDisplay');
        btn.textContent = 'Cancel';
        display.classList.add('capturing');
        display.innerHTML = '<kbd class="keycap" style="opacity:0.5">Waiting…</kbd>';

        this._keyHandler = (e) => {
            if (!this._capturing) return;
            e.preventDefault();
            e.stopPropagation();
            const mods = [];
            if (e.ctrlKey) mods.push('Ctrl');
            if (e.altKey) mods.push('Alt');
            if (e.shiftKey) mods.push('Shift');
            if (e.metaKey) mods.push('Meta');
            const key = e.key;
            if (['Control', 'Alt', 'Shift', 'Meta'].includes(key)) return;
            const keyMap = { ' ': 'Space', 'ArrowUp': 'Up', 'ArrowDown': 'Down', 'ArrowLeft': 'Left', 'ArrowRight': 'Right', 'Escape': 'Escape', 'Enter': 'Enter', 'Backspace': 'Backspace', 'Delete': 'Delete', 'Tab': 'Tab' };
            const mapped = keyMap[key] || key.toUpperCase();
            this._customHotkey = [...mods, mapped].join('+');
            this.renderKeycaps(display, this._customHotkey);
            this.stopCapture();
        };
        document.addEventListener('keydown', this._keyHandler, true);
    }

    stopCapture() {
        this._capturing = false;
        document.getElementById('captureBtn').textContent = 'Capture';
        document.getElementById('customHotkeyDisplay')?.classList.remove('capturing');
        if (this._keyHandler) {
            document.removeEventListener('keydown', this._keyHandler, true);
            this._keyHandler = null;
        }
    }

    validate() {
        if (this._selectedValue === 'custom' && (!this._customHotkey || !this._customHotkey.includes('+'))) {
            return { valid: false, error: 'Please capture a custom hotkey combination first.' };
        }
        return { valid: true };
    }

    destroy() { delete window.hotkeyModule; }
}

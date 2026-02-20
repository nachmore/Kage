/**
 * Hotkey Settings Module
 */
class HotkeySettingsModule extends SettingsModule {
    constructor() {
        super('hotkey', 'Hotkey', '🎹');
        this.isCapturing = false;
        this.capturedModifiers = [];
        this.capturedKey = '';
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                
                ${this.createSettingRow(
                    'Global Hotkey',
                    'Press the button and then press your desired hotkey combination',
                    `
                    <div style="display: flex; gap: 8px; align-items: center;">
                        <div class="hotkey-display" id="hotkeyDisplay">Alt+Space</div>
                        <button class="setting-button" id="captureBtn">Change</button>
                    </div>
                    `
                )}
            </div>
        `;
    }

    load(config) {
        const display = document.getElementById('hotkeyDisplay');
        if (display && config.hotkey) {
            display.textContent = config.hotkey.modifiers.join('+') + '+' + config.hotkey.key;
        }
    }

    save(config) {
        const display = document.getElementById('hotkeyDisplay');
        const parts = display.textContent.split('+');
        
        config.hotkey = {
            modifiers: this.capturedModifiers.length > 0 ? this.capturedModifiers : parts.slice(0, -1),
            key: this.capturedKey || parts[parts.length - 1]
        };
    }

    initialize() {
        const captureBtn = document.getElementById('captureBtn');
        if (captureBtn) {
            captureBtn.addEventListener('click', () => this.captureHotkey());
        }
    }

    captureHotkey() {
        if (this.isCapturing) {
            this.stopCapture();
            return;
        }

        this.isCapturing = true;
        this.capturedModifiers = [];
        this.capturedKey = '';
        
        const btn = document.getElementById('captureBtn');
        const display = document.getElementById('hotkeyDisplay');
        
        btn.textContent = 'Press keys...';
        display.classList.add('capturing');
        display.textContent = 'Waiting...';
        
        this.keyDownHandler = (e) => this.handleKeyCapture(e);
        this.keyUpHandler = (e) => this.handleKeyRelease(e);
        
        document.addEventListener('keydown', this.keyDownHandler);
        document.addEventListener('keyup', this.keyUpHandler);
    }

    handleKeyCapture(e) {
        if (!this.isCapturing) return;
        
        e.preventDefault();
        
        const modifiers = [];
        if (e.ctrlKey) modifiers.push('Ctrl');
        if (e.altKey) modifiers.push('Alt');
        if (e.shiftKey) modifiers.push('Shift');
        if (e.metaKey) modifiers.push('Meta');
        
        let key = e.key;
        if (!['Control', 'Alt', 'Shift', 'Meta'].includes(key)) {
            this.capturedModifiers = modifiers;
            this.capturedKey = key.toUpperCase();
            
            const display = document.getElementById('hotkeyDisplay');
            display.textContent = [...modifiers, this.capturedKey].join('+');
        }
    }

    handleKeyRelease(e) {
        if (!this.isCapturing) return;
        
        if (this.capturedKey) {
            this.stopCapture();
        }
    }

    stopCapture() {
        this.isCapturing = false;
        const btn = document.getElementById('captureBtn');
        const display = document.getElementById('hotkeyDisplay');
        btn.textContent = 'Change';
        display.classList.remove('capturing');
        
        document.removeEventListener('keydown', this.keyDownHandler);
        document.removeEventListener('keyup', this.keyUpHandler);
    }

    destroy() {
        if (this.isCapturing) {
            this.stopCapture();
        }
    }
}

/**
 * Window Walker Settings Module.
 * Reads/writes config from config.extensions['window-walker'].
 */
class WindowWalkerExtSettingsModule extends SettingsModule {
    constructor() {
        super('window-walker', 'Window Walker', '🪟');
        this.description = 'Quickly switch between open windows. Type the trigger keyword to list and filter windows.';
    }

    renderContent() {
        return `
            ${this.createControlRow(
                'Trigger keyword',
                'Type this in the floating window to activate window search.',
                '<input type="text" class="setting-input" id="wwTrigger" style="max-width:120px;" placeholder="w">'
            )}

            ${this.createCheckboxRow(
                'Show window icons',
                'Extract and display application icons next to each window. Disable for faster results on slower machines.',
                'wwShowIcons',
                true
            )}

            ${this.createCheckboxRow(
                'Hide minimized windows',
                'Exclude minimized windows from the list.',
                'wwHideMinimized',
                false
            )}
        `;
    }

    render() { return this.renderContent(); }

    load(config) {
        const c = (config.extensions && config.extensions['window-walker']) || {};
        const trigger = document.getElementById('wwTrigger');
        // Display without trailing space — it's added automatically
        if (trigger) trigger.value = (c.trigger ?? 'w ').trim();
        const icons = document.getElementById('wwShowIcons');
        if (icons) icons.checked = c.show_icons !== false;
        const hideMin = document.getElementById('wwHideMinimized');
        if (hideMin) hideMin.checked = c.hide_minimized === true;
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        // Trim user input and append space — the space is the activation delimiter
        const raw = document.getElementById('wwTrigger')?.value?.trim() || 'w';
        config.extensions['window-walker'] = {
            trigger: raw + ' ',
            show_icons: document.getElementById('wwShowIcons')?.checked ?? true,
            hide_minimized: document.getElementById('wwHideMinimized')?.checked ?? false,
        };
    }

    validate() {
        const trigger = document.getElementById('wwTrigger')?.value;
        if (!trigger || trigger.length === 0) {
            return { valid: false, error: 'Trigger keyword cannot be empty' };
        }
        return { valid: true };
    }
}
window.WindowWalkerExtSettingsModule = WindowWalkerExtSettingsModule;

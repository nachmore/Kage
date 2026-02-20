# Settings Modules

Modular settings system for Kiro Assistant.

## Structure

```
ui/js/settings/
├── base.js          # Base class for all modules
├── manager.js       # Coordinates modules and handles save/load
├── hotkey.js        # Hotkey configuration
├── connection.js    # Connection settings
├── appearance.js    # Theme and UI preferences
├── system.js        # System integration
└── README.md        # This file
```

## Adding a New Module

### 1. Create Module File

Create `ui/js/settings/yourmodule.js`:

```javascript
class YourModuleSettingsModule extends SettingsModule {
    constructor() {
        super('yourmodule', 'Your Module', '🎯');
    }

    render() {
        return `
            <div class="settings-section" id="${this.id}-section">
                <h2 class="settings-section-header">${this.icon} ${this.title}</h2>
                
                ${this.createSettingRow(
                    'Setting Label',
                    'Description of the setting',
                    '<input type="text" class="setting-input" id="mySetting">'
                )}
            </div>
        `;
    }

    load(config) {
        if (config.yourModule) {
            document.getElementById('mySetting').value = config.yourModule.setting;
        }
    }

    save(config) {
        config.yourModule = {
            setting: document.getElementById('mySetting').value
        };
    }

    validate() {
        // Optional: validate before saving
        return { valid: true };
    }

    initialize() {
        // Optional: set up event listeners
    }

    destroy() {
        // Optional: cleanup
    }
}
```

### 2. Register Module

In `manager.js`, add:

```javascript
settingsManager.registerModule(new YourModuleSettingsModule());
```

### 3. Include Script

In `settings.html`, add:

```html
<script src="js/settings/yourmodule.js"></script>
```

## Available Controls

### Text Input
```javascript
'<input type="text" class="setting-input" id="myInput">'
```

### Dropdown
```javascript
`<select class="setting-select" id="mySelect">
    <option value="opt1">Option 1</option>
    <option value="opt2">Option 2</option>
</select>`
```

### Toggle Switch
```javascript
`<label class="toggle-switch">
    <input type="checkbox" id="myToggle">
    <span class="toggle-slider"></span>
</label>`
```

### Range Slider
```javascript
`<div class="range-container">
    <input type="range" class="range-slider" id="myRange" min="0" max="100" value="50">
    <span class="range-value" id="myRangeValue">50</span>
</div>`
```

### Button
```javascript
'<button class="setting-button" id="myButton">Click Me</button>'
```

## Helper Methods

### createSettingRow(label, description, control)

Creates a properly formatted setting row:

```javascript
this.createSettingRow(
    'My Setting',
    'This is what it does',
    '<input type="text" class="setting-input" id="mySetting">'
)
```

## Module Lifecycle

1. **Construction** - `new YourModule()`
2. **Registration** - `registerModule()`
3. **Rendering** - `render()` generates HTML
4. **Initialization** - `initialize()` sets up listeners
5. **Loading** - `load(config)` populates UI
6. **Validation** - `validate()` checks inputs
7. **Saving** - `save(config)` writes to config
8. **Destruction** - `destroy()` cleans up

## Examples

See existing modules:
- `hotkey.js` - Custom input handling
- `connection.js` - Conditional UI
- `appearance.js` - Range slider
- `system.js` - Simple toggle

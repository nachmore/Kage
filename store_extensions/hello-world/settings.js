/**
 * Hello World Settings Module — sample extension.
 * Demonstrates the settings API with a text input and checkbox.
 */
class HelloWorldExtSettingsModule extends SettingsModule {
    constructor() {
        super('hello-world', 'Hello World', '👋');
        this.description = 'A sample extension. Type "test" or "hello" in the floating window to see the greeting.';
    }

    renderContent() {
        return `
            ${this.createControlRow(
                'Greeting Message',
                'The text shown when you type "test" or "hello".',
                '<input type="text" class="setting-input" id="helloGreeting" value="Hello World" style="max-width:300px;">'
            )}
            ${this.createCheckboxRow(
                'Show Timestamp',
                'Append the current time to the greeting.',
                'helloTimestamp',
                false
            )}
        `;
    }

    render() { return this.renderContent(); }

    load(config) {
        const ext = (config.extensions && config.extensions['hello-world']) || {};
        const greeting = document.getElementById('helloGreeting');
        const timestamp = document.getElementById('helloTimestamp');
        if (greeting) greeting.value = ext.greeting || 'Hello World';
        if (timestamp) timestamp.checked = ext.show_timestamp === true;
    }

    save(config) {
        if (!config.extensions) config.extensions = {};
        config.extensions['hello-world'] = {
            greeting: document.getElementById('helloGreeting')?.value || 'Hello World',
            show_timestamp: document.getElementById('helloTimestamp')?.checked ?? false,
        };
    }
}

window.HelloWorldExtSettingsModule = HelloWorldExtSettingsModule;

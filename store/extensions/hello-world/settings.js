/**
 * Hello World settings provider (sandboxed).
 * Demonstrates the schema API with a text input and a checkbox.
 */
export default class HelloWorldSettingsProvider {
    initialize(context) { this.config = context.config || {}; }
    onConfigUpdate(config) { this.config = config || {}; }

    getSettings() {
        return {
            description: 'A sample extension. Type "test" or "hello" in the floating window to see the greeting.',
            sections: [
                {
                    controls: [
                        {
                            type: 'text',
                            id: 'greeting',
                            label: 'Greeting Message',
                            description: 'The text shown when you type "test" or "hello".',
                            default: 'Hello World',
                            maxWidth: 300,
                        },
                        {
                            type: 'checkbox',
                            id: 'show_timestamp',
                            label: 'Show Timestamp',
                            description: 'Append the current time to the greeting.',
                            default: false,
                        },
                    ],
                },
            ],
        };
    }
}

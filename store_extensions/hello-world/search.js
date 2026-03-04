/**
 * Hello World search provider — sample extension.
 * Responds to "test" or "hello" with a configurable greeting.
 */
export default class HelloWorldSearchProvider {
    initialize(context) {
        this.config = context.config || {};
    }

    onConfigUpdate(config) {
        this.config = config || {};
    }

    match(query) {
        const lower = query.trim().toLowerCase();
        if (lower !== 'test' && lower !== 'hello' && !lower.startsWith('test ')) {
            return [];
        }

        const greeting = this.config.greeting || 'Hello World';
        const timestamp = this.config.show_timestamp
            ? ` (${new Date().toLocaleTimeString()})`
            : '';

        return [{
            id: 'hello-world',
            type: 'hello',
            label: greeting + timestamp,
            description: 'Press Enter to copy',
            icon: '👋',
            score: 90,
            data: { value: greeting + timestamp },
        }];
    }

    execute(result) {
        return { type: 'copy', value: result.data.value };
    }

    destroy() {}
}

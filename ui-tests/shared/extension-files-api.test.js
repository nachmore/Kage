/**
 * `context.files` in the sandbox runtime (docs/design/EXTENSION_FILE_TEXT.md
 * §3.4, §12): only with the `files.pick_text` host feature; `pickText`
 * maps its options to `pick_file_text`'s args and its `signal` to
 * `cancel_file_text`, and resolves null when the read was cancelled.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { createFilesApi } from '../../ui/js/extension-sandbox/runtime/files-api.js';

function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

describe('createFilesApi', () => {
    it('support() asks file_text_support', async () => {
        const invoke = vi.fn(async () => ({ formats: [], max_chars: 1_000_000 }));
        const files = createFilesApi({ invoke });
        expect(Object.isFrozen(files)).toBe(true);
        await files.support();
        expect(invoke).toHaveBeenCalledWith('file_text_support', {});
    });

    it('pickText() sends formats and maxChars as args, and only those given', async () => {
        const invoke = vi.fn(async () => ({ name: 'a.md' }));
        const files = createFilesApi({ invoke });
        expect(await files.pickText({ formats: ['markdown'], maxChars: 200_000 })).toEqual({
            name: 'a.md',
        });
        expect(invoke).toHaveBeenLastCalledWith('pick_file_text', {
            args: { formats: ['markdown'], max_chars: 200_000 },
        });
        await files.pickText();
        expect(invoke).toHaveBeenLastCalledWith('pick_file_text', { args: {} });
    });

    it('resolves null when nothing was picked or the read was cancelled', async () => {
        const files = createFilesApi({ invoke: vi.fn(async () => null) });
        expect(await files.pickText()).toBeNull();
        const cancelled = Object.assign(new Error('Reading a.pdf was cancelled.'), {
            kind: 'cancelled',
            key: 'errors.file_text.cancelled',
        });
        const files2 = createFilesApi({ invoke: vi.fn(async () => Promise.reject(cancelled)) });
        expect(await files2.pickText()).toBeNull();
    });

    it('other failures reject with their kind and key', async () => {
        const err = Object.assign(new Error('a.pdf is password-protected.'), {
            kind: 'invalid_input',
            key: 'errors.file_text.encrypted',
        });
        const files = createFilesApi({ invoke: vi.fn(async () => Promise.reject(err)) });
        await expect(files.pickText()).rejects.toMatchObject({
            kind: 'invalid_input',
            key: 'errors.file_text.encrypted',
        });
    });

    it('aborting the signal calls cancel_file_text; the pick then resolves null', async () => {
        const pending = deferred();
        const invoke = vi.fn((command) =>
            command === 'pick_file_text' ? pending.promise : Promise.resolve(true)
        );
        const files = createFilesApi({ invoke });
        const controller = new AbortController();
        const result = files.pickText({ signal: controller.signal });
        controller.abort();
        expect(invoke).toHaveBeenCalledWith('cancel_file_text', {});
        pending.resolve(null);
        expect(await result).toBeNull();
        // The listener is gone with the request.
        controller.signal.dispatchEvent(new Event('abort'));
        expect(invoke.mock.calls.filter(([c]) => c === 'cancel_file_text')).toHaveLength(1);
    });

    it('an aborted signal never opens anything', async () => {
        const invoke = vi.fn();
        const files = createFilesApi({ invoke });
        const controller = new AbortController();
        controller.abort();
        expect(await files.pickText({ signal: controller.signal })).toBeNull();
        expect(invoke).not.toHaveBeenCalled();
    });
});

// --- In the runtime -------------------------------------------------------------

class TextBlob {
    constructor(parts = [], opts = {}) {
        this.text = parts.map(String).join('');
        this.type = opts.type || '';
    }
}

const realBlob = globalThis.Blob;
const realCreate = URL.createObjectURL;

beforeAll(() => {
    globalThis.Blob = TextBlob;
    URL.createObjectURL = (blob) =>
        `data:text/javascript;base64,${Buffer.from(blob.text).toString('base64')}`;
});

afterAll(() => {
    globalThis.Blob = realBlob;
    URL.createObjectURL = realCreate;
});

const SEARCH_SOURCE = `
export default class Search {
    initialize(context) { globalThis.__probe.context = context; }
    match() { return []; }
}`;

async function waitFor(pred, what) {
    for (let i = 0; i < 200; i++) {
        const v = pred();
        if (v) return v;
        await new Promise((r) => setTimeout(r, 5));
    }
    throw new Error(`timed out waiting for ${what}`);
}

async function boot(hostFeatures) {
    globalThis.__probe = { context: null };
    vi.resetModules();
    await import('../../ui/js/extension-sandbox/runtime.js');
    const channel = new MessageChannel();
    const host = channel.port1;
    const inbox = [];
    host.onmessage = (ev) => inbox.push(ev.data);
    window.dispatchEvent(
        new MessageEvent('message', { data: { type: 'handshake' }, ports: [channel.port2] })
    );
    await waitFor(() => inbox.find((m) => m.type === 'ready'), 'ready');
    host.postMessage({
        type: 'init',
        config: {},
        sharedSources: {},
        hostFeatures,
        sources: { searchProvider: SEARCH_SOURCE },
    });
    await waitFor(() => inbox.find((m) => m.type === 'init-ack'), 'init-ack');
    return { host, inbox, context: globalThis.__probe.context };
}

describe('context.files in the runtime', () => {
    it('is absent without the host feature', async () => {
        const rt = await boot(['views']);
        expect(rt.context.files).toBeUndefined();
        expect(rt.context.hostFeatures).not.toContain('files.pick_text');
        rt.host.close();
    });

    it('is there with it, and its calls go to the host as invokes', async () => {
        const rt = await boot(['files.pick_text']);
        expect(rt.context.hostFeatures).toContain('files.pick_text');
        expect(typeof rt.context.files.pickText).toBe('function');
        const result = rt.context.files.pickText({ formats: ['pdf'] });
        const sent = await waitFor(() => rt.inbox.find((m) => m.type === 'invoke'), 'invoke');
        expect(sent).toMatchObject({
            command: 'pick_file_text',
            args: { args: { formats: ['pdf'] } },
        });
        rt.host.postMessage({ type: 'invoke-response', id: sent.id, result: null });
        expect(await result).toBeNull();
        rt.host.close();
    });
});

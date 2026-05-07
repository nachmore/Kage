import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { loadPrismLanguage, _resetForTests } from '../../js/shared/prism-loader.js';

// Stub out Prism with a tiny `languages` map. Each test starts with only
// 'clike' loaded so we can simulate the deps-loading path realistically.
function installFakePrism() {
    window.Prism = { languages: { clike: {} } };
}

// Replace document.head.appendChild for <script> elements: simulate a
// successful load by adding the language to Prism.languages and firing
// onload synchronously, on next microtask. Returns a `calls` array
// recording every script src that was injected.
function stubScriptInjector({ failingLanguages = new Set(), simulateRegistrationFailure = new Set() } = {}) {
    const calls = [];
    const realAppend = document.head.appendChild.bind(document.head);
    const spy = vi.spyOn(document.head, 'appendChild').mockImplementation((node) => {
        if (node.tagName === 'SCRIPT' && node.src && node.src.includes('prism-')) {
            calls.push(node.src);
            const match = node.src.match(/prism-([\w-]+)\.min\.js/);
            const lang = match ? match[1] : null;
            // Defer the onload/onerror so the awaiter sees the promise pending first
            queueMicrotask(() => {
                if (lang && failingLanguages.has(lang)) {
                    node.onerror?.(new Event('error'));
                } else {
                    if (lang && !simulateRegistrationFailure.has(lang)) {
                        window.Prism.languages[lang] = {};
                    }
                    node.onload?.(new Event('load'));
                }
            });
            return node;
        }
        return realAppend(node);
    });
    return { calls, spy };
}

beforeEach(() => {
    _resetForTests();
    installFakePrism();
});

afterEach(() => {
    delete window.Prism;
    vi.restoreAllMocks();
});

describe('loadPrismLanguage', () => {
    it('resolves synchronously when language is already loaded', async () => {
        const { calls } = stubScriptInjector();
        await loadPrismLanguage('clike');
        expect(calls).toEqual([]);
    });

    it('rejects when Prism core is not available', async () => {
        delete window.Prism;
        await expect(loadPrismLanguage('python')).rejects.toThrow(/Prism core/);
    });

    it('injects the script for an unloaded language', async () => {
        const { calls } = stubScriptInjector();
        await loadPrismLanguage('python');
        expect(calls).toHaveLength(1);
        expect(calls[0]).toMatch(/prism-python\.min\.js$/);
        expect(window.Prism.languages.python).toBeTruthy();
    });

    it('loads dependency packs first, in order', async () => {
        const { calls } = stubScriptInjector();
        // typescript → clike (already loaded), javascript, then typescript
        await loadPrismLanguage('typescript');
        expect(calls).toHaveLength(2);
        expect(calls[0]).toMatch(/prism-javascript\.min\.js$/);
        expect(calls[1]).toMatch(/prism-typescript\.min\.js$/);
    });

    it('deduplicates concurrent calls for the same language', async () => {
        const { calls } = stubScriptInjector();
        const a = loadPrismLanguage('python');
        const b = loadPrismLanguage('python');
        await Promise.all([a, b]);
        expect(calls).toHaveLength(1);
    });

    it('caches a successful load — second call is a no-op', async () => {
        const { calls } = stubScriptInjector();
        await loadPrismLanguage('python');
        await loadPrismLanguage('python');
        expect(calls).toHaveLength(1);
    });

    it('rejects when the script fails to load', async () => {
        stubScriptInjector({ failingLanguages: new Set(['xyzlang']) });
        await expect(loadPrismLanguage('xyzlang')).rejects.toThrow(/Failed to load/);
    });

    it('does NOT poison the cache on failure — retry kicks off a fresh fetch', async () => {
        const { calls } = stubScriptInjector({ failingLanguages: new Set(['xyzlang']) });
        await expect(loadPrismLanguage('xyzlang')).rejects.toThrow();
        // Second attempt should re-inject (not return the cached failed promise)
        await expect(loadPrismLanguage('xyzlang')).rejects.toThrow();
        expect(calls).toHaveLength(2);
    });

    it('rejects if the pack loads but does not register the language', async () => {
        stubScriptInjector({ simulateRegistrationFailure: new Set(['python']) });
        await expect(loadPrismLanguage('python')).rejects.toThrow(/did not register/);
    });

    it('handles deps-of-deps (typescript needs clike+javascript)', async () => {
        // Start with NO clike loaded so the full chain fires
        window.Prism.languages = {};
        const { calls } = stubScriptInjector();
        await loadPrismLanguage('typescript');
        // Match the language name from the end of the path: prism-<lang>.min.js
        expect(calls.map(s => s.match(/prism-(\w+)\.min\.js$/)[1])).toEqual(['clike', 'javascript', 'typescript']);
    });
});

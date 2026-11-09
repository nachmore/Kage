/**
 * Tests for the extension i18n proxy:
 *   - `applyManifestI18n(manifest, catalog, fallback)` resolves
 *     `__MSG_key__` tokens in the manifest's `name` and `description`
 *     against the active catalog (with EN fallback). Other manifest fields
 *     are NOT translated — they're wire data.
 *   - The runtime's `formatIcu` subset matches the host's behaviour for
 *     plurals/select/substitution; the runtime is verified by integration
 *     against `i18n.test.js` (same input, same output).
 *
 * The full sandbox integration (iframe + MessagePort) is exercised by
 * `extension-sandbox-host.test.js`; this file only covers the i18n bits.
 */

import { describe, expect, it } from 'vitest';
import {
    applyManifestI18n,
    resolveExtensionMessage,
} from '../../ui/js/shared/extension-manager.js';

describe('applyManifestI18n', () => {
    it('resolves __MSG_*__ tokens against the catalog', () => {
        const manifest = {
            id: 'demo',
            name: '__MSG_manifest.name__',
            description: '__MSG_manifest.description__',
            version: '1.0.0',
        };
        const catalog = {
            'manifest.name': { message: 'Demoテスト', description: '' },
            'manifest.description': { message: '説明', description: '' },
        };
        const out = applyManifestI18n(manifest, catalog, {});
        expect(out.name).toBe('Demoテスト');
        expect(out.description).toBe('説明');
        // Original wire fields untouched.
        expect(out.id).toBe('demo');
        expect(out.version).toBe('1.0.0');
    });

    it('falls back to EN when the active catalog is missing a key', () => {
        const manifest = { name: '__MSG_manifest.name__' };
        const catalog = {};
        const fallback = {
            'manifest.name': { message: 'English Name', description: '' },
        };
        const out = applyManifestI18n(manifest, catalog, fallback);
        expect(out.name).toBe('English Name');
    });

    it('leaves the token literal when both catalogs lack the key', () => {
        // Drift-check would have caught this in CI, but at runtime we still
        // need to return a string the user can see (and dev can grep for).
        const manifest = { name: '__MSG_missing__' };
        const out = applyManifestI18n(manifest, {}, {});
        expect(out.name).toBe('__MSG_missing__');
    });

    it('does not touch fields without the __MSG_*__ shape', () => {
        const manifest = {
            id: 'demo',
            name: 'Plain Name',
            description: 'Plain description',
            version: '1.0.0',
        };
        const out = applyManifestI18n(manifest, {}, {});
        expect(out.name).toBe('Plain Name');
        expect(out.description).toBe('Plain description');
    });

    it('returns a shallow copy — mutation is safe', () => {
        const manifest = {
            id: 'demo',
            name: '__MSG_manifest.name__',
            settings: { greeting: 'Hi' }, // nested object
        };
        const out = applyManifestI18n(
            manifest,
            { 'manifest.name': { message: 'Localised', description: '' } },
            {}
        );
        expect(out).not.toBe(manifest);
        // Nested object is shared by reference (shallow). Documented contract.
        expect(out.settings).toBe(manifest.settings);
    });

    it('only translates name and description fields', () => {
        // The convention: only user-visible labels are localised. Other fields
        // like `tags`, `permissions`, `id` are wire data.
        const manifest = {
            id: 'demo',
            name: '__MSG_manifest.name__',
            tags: ['__MSG_should_not_translate__'],
        };
        const out = applyManifestI18n(
            manifest,
            { 'manifest.name': { message: 'Localised', description: '' } },
            {}
        );
        expect(out.tags).toEqual(['__MSG_should_not_translate__']);
    });

    it('ignores a message that is not a string, so the name stays printable', () => {
        // Locale files are the extension's own JSON. `{ "toString": 1 }`
        // throws in a template literal, which would break every host text
        // that lists extension names (e.g. the agent's tool steering).
        const catalog = JSON.parse(
            '{"manifest.name": {"message": {"toString": 1}}, "manifest.description": {"message": 7}}'
        );
        const out = applyManifestI18n(
            { name: '__MSG_manifest.name__', description: '__MSG_manifest.description__' },
            catalog,
            {}
        );
        expect(out.name).toBe('__MSG_manifest.name__');
        expect(out.description).toBe('__MSG_manifest.description__');
        expect(() => `${out.name} ${out.description}`).not.toThrow();
    });
});

describe('resolveExtensionMessage', () => {
    it('returns the message, or the fallback text for a missing or non-string one', () => {
        const catalog = JSON.parse(
            '{"ok": {"message": "Notes"}, "bad": {"message": {"toString": 1}}, "num": {"message": 3}}'
        );
        expect(resolveExtensionMessage('ok', catalog, {}, 'x')).toBe('Notes');
        expect(resolveExtensionMessage('bad', catalog, {}, 'x')).toBe('x');
        expect(resolveExtensionMessage('num', catalog, {}, 'x')).toBe('x');
        expect(resolveExtensionMessage('missing', catalog, {}, 'x')).toBe('x');
    });
});

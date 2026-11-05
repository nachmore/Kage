/**
 * Effective extension config (ui/js/shared/extension-config.js): manifest
 * `config` defaults overlaid with the user's saved values, and the sparse
 * Settings save that writes them back.
 */

import { describe, expect, it } from 'vitest';
import {
    effectiveExtensionConfig,
    isExtensionEnabled,
    manifestConfigDefaults,
    mergeSettingsSave,
    resolveExtensionConfig,
} from '../../ui/js/shared/extension-config.js';

const MATH = {
    id: 'math',
    config: {
        enabled: { type: 'boolean', default: true },
        precision: { type: 'number', default: 2 },
        auto_copy: { type: 'boolean', default: true },
        thousands_separator: { type: 'boolean', default: false },
    },
};

describe('manifestConfigDefaults', () => {
    it('maps each declared key to its default, leaving out the host-owned enabled', () => {
        expect(manifestConfigDefaults(MATH)).toEqual({
            precision: 2,
            auto_copy: true,
            thousands_separator: false,
        });
    });

    it('skips entries without a default, scratch keys, and malformed blocks', () => {
        expect(
            manifestConfigDefaults({
                config: {
                    no_default: { type: 'string' },
                    __scratch: { type: 'string', default: 'x' },
                    not_an_object: 'nope',
                    empty: { type: 'string', default: '' },
                },
            })
        ).toEqual({ empty: '' });
        expect(manifestConfigDefaults({})).toEqual({});
        expect(manifestConfigDefaults(undefined)).toEqual({});
        expect(manifestConfigDefaults({ config: ['a'] })).toEqual({});
    });
});

describe('effectiveExtensionConfig', () => {
    it('nothing saved: every manifest default, plus enabled', () => {
        const expected = { precision: 2, auto_copy: true, thousands_separator: false, enabled: true };
        expect(effectiveExtensionConfig(MATH, undefined)).toEqual(expected);
        expect(effectiveExtensionConfig(MATH, null)).toEqual(expected);
        // An empty saved object is the same as none. The old runtime path
        // returned `{}` as-is, so the extension saw precision undefined.
        expect(effectiveExtensionConfig(MATH, {})).toEqual(expected);
    });

    it('partial save: saved values win, the rest are defaults', () => {
        expect(effectiveExtensionConfig(MATH, { precision: 5 })).toEqual({
            precision: 5,
            auto_copy: true,
            thousands_separator: false,
            enabled: true,
        });
    });

    it('a key a later version added gets its default for users with older saved config', () => {
        const v2 = { config: { ...MATH.config, grouping: { type: 'string', default: ',' } } };
        expect(effectiveExtensionConfig(v2, { precision: 4, auto_copy: false })).toMatchObject({
            precision: 4,
            auto_copy: false,
            grouping: ',',
        });
    });

    it('passes saved keys the manifest does not declare through unchanged', () => {
        const cfg = effectiveExtensionConfig(MATH, {
            precision: 3,
            removed_in_v2: 'kept',
            nested: { a: [1, 2] },
        });
        expect(cfg.removed_in_v2).toBe('kept');
        expect(cfg.nested).toEqual({ a: [1, 2] });
    });

    it('reads a saved value as the declared type when there is one obvious reading', () => {
        expect(
            effectiveExtensionConfig(MATH, {
                precision: '4', // a select stores strings
                auto_copy: 'false',
            })
        ).toMatchObject({ precision: 4, auto_copy: false });
        const str = { config: { label: { type: 'string', default: 'a' } } };
        expect(effectiveExtensionConfig(str, { label: 7 }).label).toBe('7');
    });

    it('ignores a saved value of the wrong type, so the default applies', () => {
        expect(
            effectiveExtensionConfig(MATH, {
                precision: 'lots',
                auto_copy: 1,
                thousands_separator: null,
            })
        ).toMatchObject({ precision: 2, auto_copy: true, thousands_separator: false });
        expect(effectiveExtensionConfig(MATH, { precision: '' }).precision).toBe(2);
        const str = { config: { label: { type: 'string', default: 'a' } } };
        expect(effectiveExtensionConfig(str, { label: { x: 1 } }).label).toBe('a');
    });

    it('passes values for keys of an unsupported declared type through', () => {
        const m = { config: { items: { type: 'array', default: [] } } };
        expect(effectiveExtensionConfig(m, { items: ['a'] }).items).toEqual(['a']);
    });

    it('ignores saved scratch values, and a JSON __proto__ key cannot reach the prototype', () => {
        const saved = JSON.parse('{"__setup_client_id":"stale","__proto__":{"polluted":true},"precision":3}');
        const cfg = effectiveExtensionConfig(MATH, saved);
        expect(cfg).not.toHaveProperty('__setup_client_id');
        expect(Object.getPrototypeOf(cfg)).toBe(Object.prototype);
        expect(cfg.polluted).toBeUndefined();
        expect(cfg.precision).toBe(3);
    });

    it('enabled is the host state; a saved enabled is ignored', () => {
        expect(effectiveExtensionConfig(MATH, { enabled: false }).enabled).toBe(true);
        expect(effectiveExtensionConfig(MATH, { enabled: true }, { enabled: false }).enabled).toBe(false);
        // Also for an extension whose manifest doesn't declare it.
        expect(effectiveExtensionConfig({}, {}, { enabled: false })).toEqual({ enabled: false });
    });

    it('returns a fresh object each time', () => {
        const saved = { precision: 3 };
        const a = effectiveExtensionConfig(MATH, saved);
        a.precision = 9;
        expect(effectiveExtensionConfig(MATH, saved).precision).toBe(3);
        expect(saved).toEqual({ precision: 3 });
    });
});

describe('resolveExtensionConfig', () => {
    it('reads saved values and enabled state from the app config', () => {
        const app = {
            extensions: { math: { precision: 6, enabled: true } },
            extension_states: { math: false },
        };
        expect(resolveExtensionConfig(app, 'math', MATH)).toEqual({
            precision: 6,
            auto_copy: true,
            thousands_separator: false,
            enabled: false,
        });
        expect(isExtensionEnabled(app, 'math')).toBe(false);
        expect(isExtensionEnabled(app, 'other')).toBe(true);
        expect(resolveExtensionConfig({}, 'math', MATH).precision).toBe(2);
    });
});

describe('mergeSettingsSave', () => {
    const page = (over = {}) => ({
        precision: 2,
        auto_copy: true,
        thousands_separator: false,
        enabled: true,
        ...over,
    });

    it('stores nothing when every value equals its default', () => {
        expect(mergeSettingsSave(MATH, undefined, page())).toEqual({});
    });

    it('stores only the values that differ from the default', () => {
        expect(mergeSettingsSave(MATH, {}, page({ precision: 4 }))).toEqual({ precision: 4 });
    });

    it('prunes an existing override that now equals the default', () => {
        // An older host wrote every control's value on Save.
        const stored = { precision: 2, auto_copy: true, thousands_separator: true };
        expect(mergeSettingsSave(MATH, stored, page())).toEqual({});
    });

    it('keeps stored keys this page has no control for', () => {
        const stored = { precision: 5, from_another_page: 'x', extension_wrote: [1] };
        // This page only shows precision.
        expect(mergeSettingsSave(MATH, stored, { precision: 3 })).toEqual({
            precision: 3,
            from_another_page: 'x',
            extension_wrote: [1],
        });
        // ...even an untouched one that happens to equal a default.
        expect(mergeSettingsSave(MATH, { auto_copy: true }, { precision: 3 })).toEqual({
            auto_copy: true,
            precision: 3,
        });
    });

    it('never stores scratch or enabled values, and removes stale ones for its controls', () => {
        const stored = { enabled: false, __setup_client_id: 'stale', __other_page: 'left' };
        expect(
            mergeSettingsSave(MATH, stored, page({ enabled: false, __setup_client_id: 'typed' }))
        ).toEqual({ __other_page: 'left' });
    });

    it('stores values for undeclared ids as is', () => {
        expect(mergeSettingsSave({}, {}, { trigger: 'sp', flag: false })).toEqual({
            trigger: 'sp',
            flag: false,
        });
    });

    it('stores a value as the declared type, and compares it with the default that way', () => {
        // A select backing a number key yields strings.
        expect(mergeSettingsSave(MATH, {}, { precision: '7' })).toEqual({ precision: 7 });
        expect(mergeSettingsSave(MATH, { precision: 7 }, { precision: '2' })).toEqual({});
        // A default written in the wrong type still matches.
        const loose = { config: { n: { type: 'number', default: '3' } } };
        expect(mergeSettingsSave(loose, {}, { n: 3 })).toEqual({});
    });

    it('does not modify the stored object it was given', () => {
        const stored = { precision: 9 };
        mergeSettingsSave(MATH, stored, { precision: 2 });
        expect(stored).toEqual({ precision: 9 });
    });

    it('round-trips: what Settings shows after a save is what was saved', () => {
        const next = mergeSettingsSave(MATH, { x: 1 }, page({ auto_copy: false }));
        expect(effectiveExtensionConfig(MATH, next)).toEqual({
            precision: 2,
            auto_copy: false,
            thousands_separator: false,
            enabled: true,
            x: 1,
        });
    });
});

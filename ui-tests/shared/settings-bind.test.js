/**
 * Tests for SettingsModule's bindFields/loadFields/saveFields DSL.
 *
 * The DSL replaces ~30 lines of repetitive `getElementById(id)?.checked
 * ?? default` plumbing per module with a single declarative spec list.
 * Three things matter:
 *   - The four `kind` values map to the right DOM read/write.
 *   - Missing config keys fall back to `default` rather than landing
 *     `undefined` in the DOM.
 *   - Save creates intermediate parents on the dotted path so a fresh
 *     config object doesn't crash on `cfg.ui.theme = …`.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { SettingsModule } from '../../ui/js/settings/base.js';

class HarnessModule extends SettingsModule {
    constructor(specs) {
        super('test', 'Test', '🧪');
        this.bindFields(specs);
    }
    render() {
        return '';
    }
    load(config) {
        this.loadFields(config);
    }
    save(config) {
        this.saveFields(config);
    }
}

beforeEach(() => {
    document.body.innerHTML = '';
});

function setupDom(html) {
    const root = document.createElement('div');
    root.innerHTML = html;
    document.body.appendChild(root);
    return root;
}

describe('bindFields — checkbox kind', () => {
    it('reads .checked into a boolean at the dotted path', () => {
        setupDom(`<input type="checkbox" id="featureToggle" checked>`);
        const m = new HarnessModule([
            { id: 'featureToggle', path: 'ui.feature', kind: 'checkbox', default: false },
        ]);
        const config = {};
        m.save(config);
        expect(config.ui.feature).toBe(true);
    });

    it('writes config value into the .checked attribute on load', () => {
        setupDom(`<input type="checkbox" id="featureToggle">`);
        const m = new HarnessModule([
            { id: 'featureToggle', path: 'ui.feature', kind: 'checkbox', default: false },
        ]);
        m.load({ ui: { feature: true } });
        expect(document.getElementById('featureToggle').checked).toBe(true);
    });

    it('falls back to `default` when the config key is missing', () => {
        setupDom(`<input type="checkbox" id="featureToggle">`);
        const m = new HarnessModule([
            { id: 'featureToggle', path: 'ui.feature', kind: 'checkbox', default: true },
        ]);
        // Config has no `ui` at all — the default should win.
        m.load({});
        expect(document.getElementById('featureToggle').checked).toBe(true);
    });

    it('coerces non-boolean config values via !!', () => {
        // Old configs sometimes have 0/1 instead of true/false.
        setupDom(`<input type="checkbox" id="x">`);
        const m = new HarnessModule([
            { id: 'x', path: 'a.b', kind: 'checkbox', default: false },
        ]);
        m.load({ a: { b: 1 } });
        expect(document.getElementById('x').checked).toBe(true);
        m.load({ a: { b: 0 } });
        expect(document.getElementById('x').checked).toBe(false);
    });
});

describe('bindFields — value/int/float kinds', () => {
    it('value: writes string value to the input on load', () => {
        setupDom(`<input type="text" id="theme">`);
        const m = new HarnessModule([
            { id: 'theme', path: 'ui.theme', kind: 'value', default: 'system' },
        ]);
        m.load({ ui: { theme: 'dark' } });
        expect(document.getElementById('theme').value).toBe('dark');
    });

    it('int: parses to integer on save', () => {
        setupDom(`<input type="number" id="fontSize" value="18">`);
        const m = new HarnessModule([
            { id: 'fontSize', path: 'ui.font_size', kind: 'int', default: 14 },
        ]);
        const config = {};
        m.save(config);
        expect(config.ui.font_size).toBe(18);
        expect(typeof config.ui.font_size).toBe('number');
    });

    it('int: empty/non-numeric input falls back to `default` on save', () => {
        setupDom(`<input type="text" id="fontSize" value="not a number">`);
        const m = new HarnessModule([
            { id: 'fontSize', path: 'ui.font_size', kind: 'int', default: 14 },
        ]);
        const config = {};
        m.save(config);
        expect(config.ui.font_size).toBe(14);
    });

    it('float: parses to floating-point on save', () => {
        setupDom(`<input type="number" id="opacity" value="0.85">`);
        const m = new HarnessModule([
            { id: 'opacity', path: 'ui.opacity', kind: 'float', default: 1.0 },
        ]);
        const config = {};
        m.save(config);
        expect(config.ui.opacity).toBeCloseTo(0.85);
    });
});

describe('bindFields — dotted paths', () => {
    it('walks multi-level paths on load', () => {
        setupDom(`<input type="text" id="a">`);
        const m = new HarnessModule([
            { id: 'a', path: 'one.two.three', kind: 'value', default: 'fallback' },
        ]);
        m.load({ one: { two: { three: 'deep' } } });
        expect(document.getElementById('a').value).toBe('deep');
    });

    it('creates missing intermediate parents on save', () => {
        // The "save into a config object that doesn't yet have the
        // section" case used to require `config.ui = config.ui ?? {}`
        // boilerplate at every site. The DSL handles it.
        setupDom(`<input type="checkbox" id="x" checked>`);
        const m = new HarnessModule([{ id: 'x', path: 'one.two.three', kind: 'checkbox' }]);
        const config = {};
        m.save(config);
        expect(config.one.two.three).toBe(true);
    });

    it('overwrites a non-object intermediate cleanly', () => {
        // A migration corner case: an old config might have
        // `cfg.ui = "system"` (string) where we now expect
        // `cfg.ui = { theme: "system" }`. The save path must clobber
        // the string with `{}` rather than crash trying to assign a
        // property on a string.
        setupDom(`<input type="text" id="theme" value="dark">`);
        const m = new HarnessModule([
            { id: 'theme', path: 'ui.theme', kind: 'value', default: 'system' },
        ]);
        const config = { ui: 'broken-old-shape' };
        m.save(config);
        expect(config.ui).toEqual({ theme: 'dark' });
    });
});

describe('bindFields — robustness', () => {
    it('skips fields whose DOM element is missing (load and save)', () => {
        // Sections render conditionally — a binding might point to
        // an id that isn't in the current DOM. Both paths must
        // tolerate that without crashing.
        const m = new HarnessModule([
            { id: 'doesNotExist', path: 'ui.x', kind: 'checkbox', default: false },
        ]);
        expect(() => m.load({ ui: { x: true } })).not.toThrow();
        const config = { ui: { x: 'preserved' } };
        m.save(config);
        // Existing config value is left alone when the DOM element is
        // absent — the alternative (overwriting with default) would
        // silently destroy data when a conditional section is hidden.
        expect(config.ui.x).toBe('preserved');
    });

    it('a module with no bind specs is a no-op', () => {
        const m = new HarnessModule(undefined);
        expect(() => m.load({})).not.toThrow();
        expect(() => m.save({})).not.toThrow();
    });
});

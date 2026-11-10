import { describe, expect, it } from 'vitest';
import { HOST_FEATURES, invokeError } from '../../ui/js/extension-sandbox/runtime/host-api.js';
import {
    ExtensionSandbox,
    HOST_FEATURE_AGENT_BACKGROUND,
    HOST_FEATURE_DATE_PICKER,
    HOST_FEATURE_SYNC_RECORDS,
    HOST_FEATURE_VIEWS,
    hostFeaturesFor,
} from '../../ui/js/shared/extension-sandbox-host.js';

describe('background agent calls (docs/design/EXTENSION_AGENT_CALLS.md §3.3)', () => {
    it('agent.background is offered only to an extension holding agent_background', () => {
        expect(HOST_FEATURE_AGENT_BACKGROUND).toBe('agent.background');
        const offered = [HOST_FEATURE_VIEWS, HOST_FEATURE_AGENT_BACKGROUND];
        expect(hostFeaturesFor(offered, ['agent_background'])).toEqual(['agent.background']);
        // The visible `agent` capability isn't enough, and nothing else is.
        expect(hostFeaturesFor(offered, ['agent', 'storage', 'views'])).toEqual(['views']);
        expect(hostFeaturesFor([HOST_FEATURE_AGENT_BACKGROUND], [])).toEqual([]);
    });

    it('a sandbox re-filters it, and a view sandbox never has it', () => {
        const spec = {
            extensionId: 'mindmap',
            capabilities: ['agent_background'],
            hostFeatures: [HOST_FEATURE_AGENT_BACKGROUND],
        };
        expect(new ExtensionSandbox(spec, () => {}, document.body).hostFeatures).toEqual([
            'agent.background',
        ]);
        const noGrant = new ExtensionSandbox(
            { ...spec, capabilities: ['storage'] },
            () => {},
            document.body
        );
        expect(noGrant.hostFeatures).toEqual([]);
        const view = new ExtensionSandbox(
            {
                ...spec,
                capabilities: ['agent_background', 'views'],
                hostFeatures: [HOST_FEATURE_AGENT_BACKGROUND, HOST_FEATURE_VIEWS],
                surface: 'view',
                viewId: 'main',
                views: [{ id: 'main', title: 'Main', module: './main.js' }],
            },
            () => {},
            document.body
        );
        expect(view.hostFeatures).toEqual(['views']);
    });
});

describe('storage change events and views (docs/design/EXTENSION_VIEWS.md §9.4)', () => {
    it('every runtime advertises storage.change_events', () => {
        expect(HOST_FEATURES).toContain('storage.change_events');
    });

    it('views is offered only to an extension holding the views capability', () => {
        expect(HOST_FEATURE_VIEWS).toBe('views');
        expect(hostFeaturesFor([HOST_FEATURE_VIEWS], ['views'])).toEqual(['views']);
        expect(hostFeaturesFor([HOST_FEATURE_VIEWS], ['storage', 'window'])).toEqual([]);
        expect(hostFeaturesFor([HOST_FEATURE_VIEWS, HOST_FEATURE_VIEWS], new Set(['views']))).toEqual([
            'views',
        ]);
        expect(
            hostFeaturesFor([HOST_FEATURE_DATE_PICKER, HOST_FEATURE_VIEWS], ['calendar', 'views'])
        ).toEqual([HOST_FEATURE_DATE_PICKER, HOST_FEATURE_VIEWS]);
        // A runtime feature isn't something a window offers.
        expect(hostFeaturesFor(['storage.change_events'], ['storage'])).toEqual([]);
        // Sync needs the sync grant (SYNC.md §12.3).
        expect(hostFeaturesFor([HOST_FEATURE_SYNC_RECORDS], ['views'])).toEqual([]);
        expect(hostFeaturesFor([HOST_FEATURE_SYNC_RECORDS], ['sync', 'views'])).toEqual([
            HOST_FEATURE_SYNC_RECORDS,
        ]);
    });
});

describe('sandbox runtime host API', () => {
    it('advertises the settings copy action, structured invoke errors, manifest-default config, minimized windows, and atomic data writes', () => {
        expect(HOST_FEATURES).toContain('settings.copy_action');
        expect(HOST_FEATURES).toContain('invoke.error_kind');
        expect(HOST_FEATURES).toContain('config.manifest_defaults');
        expect(HOST_FEATURES).toContain('windows.minimized');
        expect(HOST_FEATURES).toContain('export.sensitive_data');
        expect(HOST_FEATURES).toContain('export.plain_settings');
        expect(HOST_FEATURES).toContain('storage.atomic_write');
        expect(Object.isFrozen(HOST_FEATURES)).toBe(true);
    });

    it('lists each feature once', () => {
        expect(new Set(HOST_FEATURES).size).toBe(HOST_FEATURES.length);
    });

    it('builds an Error carrying the AppError kind and key', () => {
        const err = invokeError({
            error: 'OAuth listener was cancelled before a callback arrived',
            errorKind: 'cancelled',
            errorKey: 'errors.passthrough',
        });
        expect(err).toBeInstanceOf(Error);
        expect(err.message).toBe('OAuth listener was cancelled before a callback arrived');
        expect(err.kind).toBe('cancelled');
        expect(err.key).toBe('errors.passthrough');
    });

    it('leaves kind / key unset for plain host errors', () => {
        const err = invokeError({ error: "Extension 'x': missing capability 'oauth'" });
        expect(err.message).toMatch(/missing capability/);
        expect(err).not.toHaveProperty('kind');
        expect(err).not.toHaveProperty('key');
    });
});

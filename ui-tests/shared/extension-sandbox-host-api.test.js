import { describe, expect, it } from 'vitest';
import { HOST_FEATURES, invokeError } from '../../ui/js/extension-sandbox/runtime/host-api.js';
import {
    HOST_FEATURE_DATE_PICKER,
    HOST_FEATURE_SYNC_RECORDS,
    HOST_FEATURE_VIEWS,
    hostFeaturesFor,
} from '../../ui/js/shared/extension-sandbox-host.js';

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
        // Sync isn't offered yet (WP-S5).
        expect(hostFeaturesFor([HOST_FEATURE_SYNC_RECORDS], ['sync', 'views'])).toEqual([]);
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

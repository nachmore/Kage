import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
    matchesSensitiveData,
    normalizeSensitiveData,
    plainBackupFields,
    SENSITIVE_DATA_MAX_ENTRIES,
    SENSITIVE_NAME_RE,
    sensitiveConfigFields,
} from '../../ui/js/shared/extension-sensitive-data.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

afterEach(() => vi.restoreAllMocks());

describe('normalizeSensitiveData', () => {
    it('keeps exact keys and prefix globs, lowercased and deduplicated', () => {
        expect(normalizeSensitiveData(['creds', 'token_*', 'Creds', '*'], 'x')).toEqual([
            'creds',
            'token_*',
            '*',
        ]);
    });

    it('drops malformed entries with a warning, like unknown capabilities', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const out = normalizeSensitiveData(
            ['ok', 42, null, '', '../escape', 'a/b', 'spaces here', 'to*ken', '**', 'a..b*', 'x'.repeat(129), 'fine_*'],
            'x'
        );
        expect(out).toEqual(['ok', 'fine_*']);
        expect(warn).toHaveBeenCalled();
    });

    it('ignores a value that is not an array', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        expect(normalizeSensitiveData('creds', 'x')).toEqual([]);
        expect(normalizeSensitiveData({ creds: true }, 'x')).toEqual([]);
        expect(normalizeSensitiveData(undefined, 'x')).toEqual([]);
        expect(normalizeSensitiveData(null, 'x')).toEqual([]);
        expect(warn).toHaveBeenCalledTimes(2);
    });

    it(`reads at most ${SENSITIVE_DATA_MAX_ENTRIES} entries`, () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const raw = Array.from({ length: 40 }, (_, i) => `key${i}`);
        const out = normalizeSensitiveData(raw, 'x');
        expect(out).toHaveLength(32);
        expect(out.at(-1)).toBe('key31');
    });
});

describe('matchesSensitiveData', () => {
    const patterns = normalizeSensitiveData(['creds', 'token_*'], 'x');
    it('matches exact keys and prefixes, ignoring case', () => {
        expect(matchesSensitiveData('creds', patterns)).toBe(true);
        expect(matchesSensitiveData('CREDS', patterns)).toBe(true);
        expect(matchesSensitiveData('token_refresh', patterns)).toBe(true);
        expect(matchesSensitiveData('creds2', patterns)).toBe(false);
        expect(matchesSensitiveData('my_token_x', patterns)).toBe(false);
        expect(matchesSensitiveData('anything', ['*'])).toBe(true);
    });
});

describe('sensitiveConfigFields', () => {
    it('needs a boolean true', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const fields = sensitiveConfigFields({
            id: 'x',
            config: {
                api_key: { type: 'string', default: '', sensitive: true },
                calendar_url: { type: 'string', sensitive: true },
                units: { type: 'string', sensitive: false },
                sloppy: { type: 'string', sensitive: 'yes' },
                plain: { type: 'boolean', default: true },
            },
        });
        expect(fields.sort()).toEqual(['api_key', 'calendar_url']);
        expect(warn).toHaveBeenCalledTimes(1);
        expect(sensitiveConfigFields({})).toEqual([]);
        expect(sensitiveConfigFields(undefined)).toEqual([]);
    });
});

describe('plainBackupFields', () => {
    it('needs backup "plain", and sensitive wins', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const fields = plainBackupFields({
            id: 'x',
            config: {
                city: { type: 'string', backup: 'plain' },
                key: { type: 'string', backup: 'plain', sensitive: true },
                odd: { type: 'string', backup: 'yes' },
                units: { type: 'string', enum: ['a', 'b'] },
            },
        });
        expect(fields).toEqual(['city']);
        expect(warn).toHaveBeenCalledTimes(2);
        warn.mockRestore();
        expect(plainBackupFields({})).toEqual([]);
    });
});

describe('safety net', () => {
    it('matches the contract names', () => {
        for (const name of [
            'token', 'refresh_token', 'SECRET', 'credentials', 'creds', 'password', 'passwd',
            'api_key', 'api-key', 'apikey', 'ApiKey', 'auth', 'oauth_state', 'session', 'cookie_jar',
            'passphrase', 'pin', 'wifi_pass', 'db_pwd', 'licenseKey', 'API Key', 'totp_seed',
            'AccountKey',
        ]) {
            expect(SENSITIVE_NAME_RE.test(name), name).toBe(true);
        }
        for (const name of [
            'todos', 'bookmarks', 'location', 'watchlist', 'units', 'trigger', 'pinned', 'spinner',
            'bypass', 'compass', 'hotkey',
        ]) {
            expect(SENSITIVE_NAME_RE.test(name), name).toBe(false);
        }
    });

    it('stays in sync with the Rust pattern and entry limit', () => {
        const rust = readFileSync(path.join(repoRoot, 'src', 'extensions.rs'), 'utf8');
        const pattern = rust.match(/SENSITIVE_NAME_PATTERN: &str =\s*r"\(\?i\)([^"]+)"/);
        expect(pattern, 'SENSITIVE_NAME_PATTERN in src/extensions.rs').not.toBeNull();
        expect(SENSITIVE_NAME_RE.source).toBe(pattern[1]);
        expect(SENSITIVE_NAME_RE.flags).toBe('i');
        const max = rust.match(/MAX_SENSITIVE_DATA_ENTRIES: usize = (\d+);/);
        expect(Number(max?.[1])).toBe(SENSITIVE_DATA_MAX_ENTRIES);
    });
});

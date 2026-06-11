/**
 * Guard: nobody registers a raw `config_updated` listener that reads
 * config, bypassing the cache's invalidation ordering.
 *
 * The bug this prevents: config-cache.js memoises get_config and clears
 * itself on the `config_updated` event. If another module ALSO listens
 * for `config_updated` and calls getConfig() in its handler, the two
 * listeners race — Tauri dispatches them in an unspecified order, and if
 * the consumer runs first it reads the stale cache. That's how a
 * newly-added quick command failed to appear in floating-window search
 * until the next app launch.
 *
 * The fix was to route config-change reactions through
 * `onConfigChange()` (in config-cache.js), which invalidates the cache
 * BEFORE notifying subscribers. This test stops the raw pattern from
 * creeping back: any `listen(... 'config_updated' ...)` in ui/js outside
 * the allow-listed files fails here, with a pointer to onConfigChange.
 *
 * Allow-listed:
 *   - config-cache.js — owns the single authoritative listener.
 *   - i18n.js — listens to refetch the i18n catalog via a *different*
 *     backend command (get_i18n_catalog), not the config cache, so it
 *     can't observe a stale getConfig.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const uiJsRoot = path.join(repoRoot, 'ui', 'js');

// Files permitted to reference config_updated in a listen() call.
const ALLOWED = new Set([
    path.join('shared', 'config-cache.js'),
    path.join('shared', 'i18n.js'),
]);

/** Recursively collect every .js file under ui/js. */
function jsFiles(dir) {
    const out = [];
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            out.push(...jsFiles(full));
        } else if (entry.isFile() && entry.name.endsWith('.js')) {
            out.push(full);
        }
    }
    return out;
}

// Matches a listen(...) call whose arguments mention config_updated,
// whether passed as the EVT.CONFIG_UPDATED constant or a raw string.
// Spans the call across newlines (handler bodies are multi-line).
const LISTEN_CONFIG_UPDATED =
    /\blisten\s*\(\s*(?:EVT\.CONFIG_UPDATED|['"]config_updated['"])/;

describe('config_updated listener discipline', () => {
    it('no raw config_updated listeners outside the allow-list', () => {
        const offenders = [];
        for (const file of jsFiles(uiJsRoot)) {
            const rel = path.relative(uiJsRoot, file);
            if (ALLOWED.has(rel)) continue;
            const text = readFileSync(file, 'utf8');
            if (LISTEN_CONFIG_UPDATED.test(text)) {
                offenders.push(rel);
            }
        }

        expect(
            offenders,
            `These files register a raw config_updated listener:\n  ${offenders.join('\n  ')}\n\n` +
                `Use onConfigChange(handler) from shared/config-cache.js instead. It runs your\n` +
                `handler AFTER the config cache is invalidated, so a getConfig() inside it sees\n` +
                `fresh data. A raw listener can fire before the cache clears and read stale config\n` +
                `(the "new shortcut doesn't show up until restart" bug). If your handler genuinely\n` +
                `does NOT read config (e.g. it calls a different backend command), add the file to\n` +
                `the ALLOWED set in this test with a comment explaining why.`,
        ).toEqual([]);
    });
});

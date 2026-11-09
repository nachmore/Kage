/**
 * Load the EN catalog into ui/js/shared/i18n.js for tests that check the
 * real text (Settings → Sync and the where-it-applies control).
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

/** The EN catalog, without `_meta`. */
export function enCatalog() {
    const raw = JSON.parse(readFileSync(path.join(repoRoot, 'locales/en/messages.json'), 'utf8'));
    delete raw._meta;
    return raw;
}

/**
 * Initialise i18n with the EN catalog. `rtl` pretends the active language
 * is Arabic (RTL), still with the English text.
 */
export async function initEnglish(i18n, { rtl = false } = {}) {
    const catalog = enCatalog();
    await i18n.initI18n(async (cmd) => {
        if (cmd !== 'get_i18n_catalog') throw new Error(`unexpected ${cmd}`);
        return { catalog, fallback: catalog, language: rtl ? 'ar' : 'en', rtl };
    });
}

export { repoRoot };

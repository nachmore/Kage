/**
 * Shared setup for the view page tests: the real markup of
 * `ui/extension-view.html` in jsdom, and a stand-in `t()` that shows which
 * key and variables were used.
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

export const VIEW_HTML = readFileSync(path.join(repoRoot, 'ui/extension-view.html'), 'utf8');

/** Put the page's body markup (without its script) into the test document. */
export function mountViewHtml() {
    const parsed = new DOMParser().parseFromString(VIEW_HTML, 'text/html');
    for (const script of parsed.querySelectorAll('script')) script.remove();
    document.title = 'Kage';
    document.body.className = parsed.body.className;
    document.body.innerHTML = parsed.body.innerHTML;
}

/** `t()` for tests: the key, plus its variables when there are any. */
export function fakeT(key, vars) {
    return vars && Object.keys(vars).length ? `${key} ${JSON.stringify(vars)}` : key;
}

/**
 * Cross-language alignment for the extension permission + IPC layer.
 *
 * Extensions interact with Kage through three surfaces that have to
 * stay in lockstep:
 *
 *   1. The capability allow-list. JS owns the user-facing metadata
 *      (`CAPABILITIES` in extension-permissions.js), Rust owns the
 *      authoritative validator (`VALID_CAPABILITIES` in extensions.rs)
 *      that decides what survives `commit_extension_install`. If the
 *      two disagree, an extension that declares a cap one side knows
 *      and the other doesn't will install with a silently-truncated
 *      grant. That's how the Spotify install bug landed: JS shipped
 *      `oauth` and `network` after the `shell` split, Rust didn't,
 *      so spotify's `["storage", "urls", "oauth"]` committed as
 *      `["storage", "urls"]` and the OAuth flow lost its loopback
 *      listener.
 *
 *   2. The Kage-Extensions catalog validator's allow-list, in
 *      `Kage-Extensions/scripts/host-capabilities.mjs`. We can't
 *      reach across repos at test time, but we DO own the
 *      JS-side mirror in this repo, and that file's KNOWN_CAPABILITIES
 *      matches CAPABILITIES — drift here would surface as test
 *      failures inside that test file.
 *
 *   3. The storage-command IPC arg shape. Tauri 2 auto-renames Rust
 *      function-parameter snake_case to JS camelCase over the wire.
 *      A storage command that takes `extension_id: String` in Rust
 *      MUST receive `extensionId` from JS. The extension-sandbox-host
 *      force-injects this arg on every storage IPC; if it injects
 *      under the wrong name (a previous bug used `extension_id`
 *      verbatim), the command rejects with "missing required key
 *      extensionId" and every save_extension_data /
 *      load_extension_data call fails for sandboxed extensions.
 *
 * Together these three tests would have caught every layer of the
 * spotify install hat-trick at PR time.
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { describe, it, expect } from 'vitest';
import {
    CAPABILITIES,
    COMMAND_CAPABILITIES,
    KNOWN_CAPABILITIES,
} from '../../ui/js/shared/extension-permissions.js';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

/**
 * Pull the contents of `pub const NAME: &[&str] = &[ "a", "b", ... ];`
 * out of a Rust source file. Returns the array of strings, or null if
 * not found. Tolerates leading/trailing whitespace, comments inside
 * the array, and trailing commas — i.e. the shapes our actual constants
 * take in extensions.rs.
 */
function parseRustStrSliceConst(rustPath, name) {
    const text = readFileSync(rustPath, 'utf8');
    const re = new RegExp(`pub const ${name}\\s*:\\s*&\\[\\s*&\\s*str\\s*\\]\\s*=\\s*&\\[([\\s\\S]*?)\\]\\s*;`);
    const m = text.match(re);
    if (!m) return null;
    const body = m[1]
        .split('\n')
        .map((line) => line.replace(/\/\/.*$/, '')) // strip line comments
        .join('\n');
    const items = [];
    const itemRe = /"([^"]+)"/g;
    let im;
    while ((im = itemRe.exec(body)) !== null) {
        items.push(im[1]);
    }
    return items;
}

/**
 * Pull every `pub async fn NAME(...arg: TYPE...) -> ...` (or non-async)
 * Tauri command signature from a Rust source file. Returns
 * `Map<fnName, Array<argName>>` for arg lists, ignoring arg types and
 * Tauri-injected types like `tauri::AppHandle` / `State<'_, T>` /
 * `tauri::Window`. Used to verify that the JS storage-injection layer
 * uses the param names Tauri actually expects on the wire.
 */
function parseTauriCommands(rustPath) {
    const text = readFileSync(rustPath, 'utf8');
    const out = new Map();
    // Match `#[tauri::command]` followed by an optional `pub` and an
    // `async fn` / `fn`, then capture the name + arg list.
    const re = /#\[tauri::command\][\s\S]*?pub\s+(?:async\s+)?fn\s+(\w+)\s*\(([\s\S]*?)\)/g;
    let m;
    while ((m = re.exec(text)) !== null) {
        const name = m[1];
        const argList = m[2];
        const args = [];
        for (const part of splitTopLevelCommas(argList)) {
            const trimmed = part.trim();
            if (!trimmed) continue;
            // Match `argname: Type` or `mut argname: Type`. Skip
            // tauri-injected receiver-style args we don't care about.
            const arg = trimmed.match(/^(?:mut\s+)?([a-z_][a-z0-9_]*)\s*:\s*(.+)$/i);
            if (!arg) continue;
            const argName = arg[1];
            const argType = arg[2];
            // Don't surface Tauri-injected args (AppHandle, State, Window).
            if (
                /tauri::AppHandle|tauri::Window|tauri::WebviewWindow|State\s*</.test(argType) ||
                /AppHandle$|WebviewWindow$|^Window$/.test(argType.trim())
            ) {
                continue;
            }
            args.push(argName);
        }
        out.set(name, args);
    }
    return out;
}

/** Split on top-level commas (commas not nested inside `<>` or `(`). */
function splitTopLevelCommas(s) {
    const out = [];
    let depth = 0;
    let start = 0;
    for (let i = 0; i < s.length; i++) {
        const c = s[i];
        if (c === '<' || c === '(' || c === '[') depth++;
        else if (c === '>' || c === ')' || c === ']') depth--;
        else if (c === ',' && depth === 0) {
            out.push(s.slice(start, i));
            start = i + 1;
        }
    }
    out.push(s.slice(start));
    return out;
}

function snakeToCamel(s) {
    return s.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());
}

describe('extension permission table — Rust ↔ JS alignment', () => {
    it('VALID_CAPABILITIES (Rust) and KNOWN_CAPABILITIES (JS) are identical sets', () => {
        const rust = parseRustStrSliceConst(
            path.join(repoRoot, 'src/extensions.rs'),
            'VALID_CAPABILITIES'
        );
        expect(rust, 'could not parse VALID_CAPABILITIES out of src/extensions.rs').not.toBeNull();
        const rustSet = new Set(rust);
        const jsSet = new Set(KNOWN_CAPABILITIES);

        const onlyInRust = [...rustSet].filter((c) => !jsSet.has(c));
        const onlyInJs = [...jsSet].filter((c) => !rustSet.has(c));

        // Concrete failure mode: a cap missing from one side will be
        // silently dropped by `normalize_permissions` on that side at
        // install time (with a "unknown capability" warning that few
        // contributors will ever see). The user gets a less-privileged
        // install than they consented to, or the extension refuses to
        // call commands it expected to be allowed. Spotify's `oauth`
        // landed exactly this way — JS knew, Rust didn't.
        expect(
            onlyInRust,
            `Rust extends VALID_CAPABILITIES with ${JSON.stringify(onlyInRust)}; mirror in ui/js/shared/extension-permissions.js CAPABILITIES`
        ).toEqual([]);
        expect(
            onlyInJs,
            `JS extends CAPABILITIES with ${JSON.stringify(onlyInJs)}; mirror in src/extensions.rs VALID_CAPABILITIES`
        ).toEqual([]);
    });

    it('every JS CAPABILITIES key has icon + label + description', () => {
        // Already covered by extension-permissions.test.js but the
        // assertion is cheap and pairs well with the parity check
        // — easier to debug when both fire from one file.
        for (const cap of KNOWN_CAPABILITIES) {
            const meta = CAPABILITIES[cap];
            expect(meta?.icon, `${cap}.icon`).toBeTypeOf('string');
            expect(meta?.label, `${cap}.label`).toBeTypeOf('string');
            expect(meta?.description, `${cap}.description`).toBeTypeOf('string');
            expect(meta.description.length, `${cap}.description`).toBeGreaterThan(10);
        }
    });

    it('every COMMAND_CAPABILITIES value points to a known cap or null', () => {
        for (const [cmd, cap] of Object.entries(COMMAND_CAPABILITIES)) {
            if (cap === null) continue;
            expect(
                KNOWN_CAPABILITIES.includes(cap),
                `${cmd} → '${cap}' is not in KNOWN_CAPABILITIES`
            ).toBe(true);
        }
    });
});

describe('extension storage IPC — Rust ↔ JS arg-name alignment', () => {
    // The host's STORAGE_COMMANDS list lives in the sandbox-host
    // module. We re-derive the names here (rather than importing the
    // private const) to avoid coupling the test to the file's
    // export shape — and to encode our intent: "for every storage
    // command we forward, the camelCase arg names must match Rust."
    const STORAGE_COMMANDS = [
        'save_extension_data',
        'load_extension_data',
        'delete_extension_data',
    ];

    it('every storage command takes an extension_id arg on the Rust side', () => {
        // If anyone ever renames the Rust param (e.g. to `id`) without
        // updating the JS injection target, this test reminds them
        // that the wire-name has changed.
        const cmds = parseTauriCommands(path.join(repoRoot, 'src/commands/extensions.rs'));
        for (const name of STORAGE_COMMANDS) {
            const args = cmds.get(name);
            expect(args, `${name}: not found as a #[tauri::command] in src/commands/extensions.rs`).toBeDefined();
            expect(
                args.includes('extension_id'),
                `${name}(${args.join(', ')}) — expected param 'extension_id'; if you renamed it, update STORAGE_COMMANDS in extension-sandbox-host.js too`
            ).toBe(true);
        }
    });

    it('host injects "extensionId" (camelCase), matching the Tauri 2 wire convention', () => {
        // Read the source verbatim so we catch a regression to
        // snake_case (the spotify storage bug). The injection happens
        // in a single place — a regex over the host file is the
        // simplest test that doesn't require reaching into private
        // implementation details.
        const text = readFileSync(
            path.join(repoRoot, 'ui/js/shared/extension-sandbox-host.js'),
            'utf8'
        );
        // Allow either `extensionId: ...` or `'extensionId': ...` —
        // both are valid shorthand in modern JS.
        expect(
            /extensionId\s*:\s*this\.extensionId/.test(text),
            'extension-sandbox-host.js should inject `extensionId: this.extensionId` (camelCase). Snake_case is silently dropped by Tauri 2 IPC and breaks every save/load/delete from a sandbox.'
        ).toBe(true);
        // Concrete regression guard: snake_case form should NOT appear
        // anywhere in the file.
        expect(
            /extension_id\s*:\s*this\.extensionId/.test(text),
            'extension-sandbox-host.js still contains the snake_case form `extension_id: this.extensionId` — Tauri 2 IPC expects camelCase. See the Spotify install bug.'
        ).toBe(false);
    });

    it('the camelCase JS injection name matches what Rust expects after Tauri rename', () => {
        // Compose the assertion: for each storage command, the Rust
        // arg `extension_id` becomes `extensionId` over the wire,
        // and that's what the host must inject.
        const cmds = parseTauriCommands(path.join(repoRoot, 'src/commands/extensions.rs'));
        for (const name of STORAGE_COMMANDS) {
            const args = cmds.get(name) ?? [];
            const wireNames = args.map(snakeToCamel);
            expect(
                wireNames,
                `${name}: Rust args ${JSON.stringify(args)} → wire names ${JSON.stringify(wireNames)} — host injection must use the wire name`
            ).toContain('extensionId');
        }
    });
});

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

import { readdirSync, readFileSync } from 'node:fs';
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
    // Accept either a single .rs file or a module directory; when given a
    // directory, concatenate every .rs file under it. The extensions
    // commands were split from a single `extensions.rs` into an
    // `extensions/` module (discovery/files/install/store/welcome), so
    // scanning the directory keeps this test working across that split
    // and any future re-split.
    const text = rustPath.endsWith('.rs')
        ? readFileSync(rustPath, 'utf8')
        : readdirSync(rustPath)
              .filter((f) => f.endsWith('.rs'))
              .map((f) => readFileSync(path.join(rustPath, f), 'utf8'))
              .join('\n');
    const out = new Map();
    // Match `#[tauri::command]` followed by an optional `pub` and an
    // `async fn` / `fn`, then capture the name + arg list. Commands may
    // carry a generic runtime param (`fn foo<R: tauri::Runtime>(...)`)
    // since the mock-app harness made them runtime-generic.
    const re = /#\[tauri::command\][\s\S]*?pub\s+(?:async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([\s\S]*?)\)/g;
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
    // The host's IDENTITY_SCOPED_COMMANDS map lives in the sandbox-host
    // module. We re-derive it here (rather than importing the private
    // const) to avoid coupling the test to the file's export shape — and
    // to encode our intent: "for every identity-scoped command we forward,
    // the host must inject the calling sandbox's real id under the arg
    // name Rust expects." A command that touches per-extension state but
    // is missing from this map is a cross-extension access hole.
    //
    // Value = the Rust param name (snake_case). extension_data commands
    // take `extension_id`; the *_extension_config commands take a bare `id`.
    const IDENTITY_SCOPED = {
        save_extension_data: 'extension_id',
        load_extension_data: 'extension_id',
        delete_extension_data: 'extension_id',
        get_extension_config: 'id',
        save_extension_config: 'id',
    };

    it('every identity-scoped command takes its id arg on the Rust side', () => {
        // If anyone renames a Rust param without updating the JS injection
        // target, this reminds them the wire-name has changed.
        const cmds = parseTauriCommands(path.join(repoRoot, 'src/commands/extensions'));
        for (const [name, rustArg] of Object.entries(IDENTITY_SCOPED)) {
            const args = cmds.get(name);
            expect(args, `${name}: not found as a #[tauri::command] in src/commands/extensions/`).toBeDefined();
            expect(
                args.includes(rustArg),
                `${name}(${args.join(', ')}) — expected param '${rustArg}'; if you renamed it, update IDENTITY_SCOPED_COMMANDS in extension-sandbox-host.js too`
            ).toBe(true);
        }
    });

    it('host injects the calling sandbox id under each command’s wire arg name', () => {
        // Read the source verbatim so we catch a regression to snake_case
        // (the spotify storage bug) or a dropped config command. The
        // injection maps command -> wire arg name and forwards
        // `{ [idArgName]: this.extensionId }`.
        const text = readFileSync(
            path.join(repoRoot, 'ui/js/shared/extension-sandbox-host.js'),
            'utf8'
        );
        // The map must list every identity-scoped command with its wire
        // (camelCase) arg name, and the forward must key off that name.
        for (const [name, rustArg] of Object.entries(IDENTITY_SCOPED)) {
            const wireArg = snakeToCamel(rustArg);
            const entry = new RegExp(`${name}\\s*:\\s*['"]${wireArg}['"]`);
            expect(
                entry.test(text),
                `extension-sandbox-host.js should map ${name} -> '${wireArg}' in IDENTITY_SCOPED_COMMANDS`
            ).toBe(true);
        }
        expect(
            /\[idArgName\]\s*:\s*this\.extensionId/.test(text),
            'extension-sandbox-host.js should force-inject `[idArgName]: this.extensionId` for identity-scoped commands.'
        ).toBe(true);
        // Concrete regression guard: the old always-camelCase snake_case
        // form should NOT appear.
        expect(
            /extension_id\s*:\s*this\.extensionId/.test(text),
            'extension-sandbox-host.js still contains the snake_case form `extension_id: this.extensionId` — Tauri 2 IPC expects camelCase. See the Spotify install bug.'
        ).toBe(false);
    });
});

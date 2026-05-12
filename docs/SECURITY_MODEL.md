# Security Model

## The trust boundary

Kage treats every extension as untrusted. That includes extensions
bundled with the app — the same sandbox, permission grant, and IPC
enforcement apply to bundled and user-installed extensions alike.

The first-party UI (HTML/JS/CSS shipped inside the binary) is trusted.
Everything else, including every extension's search / tool / trigger
provider, runs in an isolated iframe with no access to Tauri.

## How extensions are sandboxed

Each extension's provider code is loaded into its own iframe using
`sandbox="allow-scripts"`. Without `allow-same-origin`, the iframe gets
a **unique null origin**, which means:

- No `window.__TAURI__` global — Tauri's API is not injected.
- No access to the parent window, its cookies, or its localStorage.
- No access to any other extension's iframe.
- All IPC with the host goes through a single `MessagePort` opened
  during a handshake, and that channel is the only way the extension
  can reach the outside world.

The host (`ui/js/shared/extension-sandbox-host.js`) authoritatively
enforces the capability check on every `invoke()` request arriving
over the port. The extension cannot spoof its identity — the host
created the iframe, so the host knows which extension owns which
port.

### Source loading

Extension JavaScript is fetched by the **host** (via
`read_extension_file` for user extensions, or a local `fetch` for
bundled ones). The host passes the resulting source text to the
sandbox through the message port, where it's imported via a blob URL
local to the iframe. The sandbox never makes a network request to
the host's origin — it only imports from blob URLs it received.

### Vendor libraries (allow-listed UMD/IIFE)

Some extensions depend on a non-ESM library that exposes a global
(e.g. mathjs). Extensions declare these in their manifest via
`sandboxVendor`, but the names are resolved against a fixed
allow-list in `extension-manager.js` — extensions can't name
arbitrary paths. The host fetches the pre-bundled file and passes
the source text to the sandbox runtime, which caches it and makes
it available to `runSandboxed` — the helper that spawns a
terminable Web Worker for extension compute. Vendor libraries are
not loaded into the sandbox iframe itself, so a runaway vendor
call can always be killed by terminating the Worker. See
`docs/EXTENSIONS.md#vendor-libraries` for the current allow-list.

The trust model is the same as for provider code: the vendor runs
inside the null-origin Worker, so even a compromised vendor can't
touch Tauri. The host never downloads vendors from the network at
runtime — they ship with the app.

### Which contribution points are sandboxed today

| Contribution point     | In sandbox?        |
|------------------------|--------------------|
| Search provider        | ✅ Yes             |
| Tool provider          | ✅ Yes             |
| Trigger provider       | ✅ Yes             |
| Settings provider      | ✅ Yes — declarative schema; see EXTENSIONS.md |
| Toolbar button         | ✅ Yes — declarative button definitions with RPC on-click handlers |
| Message formatter      | ✅ Yes — host hands the rendered HTML to the sandbox; sandbox returns replacement HTML which is sanitized before injection |
| Widget                 | ✅ Yes — widget returns HTML + action declarations on a refresh interval the extension controls |
| Custom `renderResult`  | ✅ Yes — sandbox returns sanitized HTML for individual result rows |

The disabled contribution points don't break an extension's install or
its other providers; they just don't execute. Extensions that relied
on them should expect those features to be inert until follow-up
commits land.

## Capabilities and the install-time grant

Every Tauri IPC command reachable from an extension is mapped to
exactly one **capability**, or to `null` if it's never callable from
an extension (examples: `save_config`, `quit_app`,
`execute_system_command`, `update_tool_policy`). The complete map
lives in
[`ui/js/shared/extension-permissions.js`](../ui/js/shared/extension-permissions.js).

An extension's manifest declares the capabilities it needs:

```json
{
  "id": "my-extension",
  "permissions": ["storage", "shell"]
}
```

At install time, the user sees the declared set in a modal listing
each capability's icon, label, and description. The user either
approves the whole set or cancels; there is no partial approval
today.

### Install flow

There are two install paths, both of which end with a recorded grant
in `config.extension_grants`. The trust invariant — a grant only ever
contains capabilities that the user has (implicitly or explicitly)
accepted, normalised against the known-capabilities list — holds
across both.

#### Path A: Store / manual install (modal-per-extension)

Used when the user installs from the extension store, drags in a
local zip or directory, or when an upgrade requests new capabilities.

1. User clicks **Install** in the store (or equivalent).
2. `store_install` / `install_extension_from_path` downloads the
   archive, extracts it to the user extension directory, and enables
   the extension in config — but does **not** emit
   `extensions_changed`, so the loader doesn't pick it up yet.
3. The frontend reads the installed manifest and opens the
   permission prompt.
4. If the user approves: `commit_extension_install` saves the
   grant record and emits `extensions_changed`, which fires the
   loader and the sandbox boots with the approved capability set.
5. If the user cancels: `uninstall_extension` rolls back, and
   nothing ever got loaded.

#### Path B: First-run welcome (batch approval, no modal)

Used for the extensions shown on the welcome screen, drawn from
`ui/extensions/recommended.json`. The welcome screen renders each
extension's declared capabilities as pills on the card, so the user
sees exactly what each box they tick is agreeing to. Clicking
**Finish** is the approval signal for every ticked box.

1. Welcome screen renders each extension card with capability pills
   sourced from the manifest's `permissions` field.
2. User ticks/unticks boxes to opt in/out.
3. User clicks **Finish**. First-run config + telemetry consent
   apply synchronously; the window hides.
4. For each ticked non-bundled extension, the renderer calls
   `install_and_commit_bundled(id)`. The Rust handler:
   - Verifies a matching zip exists in the bundled packages
     directory. If not, the call is rejected with an explicit error
     identifying the security boundary. **This is what stops a
     compromised welcome page from batch-granting caps to arbitrary
     store extensions.**
   - Extracts the zip and reads the on-disk manifest.
   - Pulls the capability list from the manifest (not the renderer)
     and normalises it through `extensions::normalize_permissions`.
   - Writes the grant and emits `extensions_changed` atomically.

Why this path is safe to skip the modal: the zip lives inside our
signed installer, the capability list comes off the extracted
manifest (not the renderer), and the user has already seen each
extension's cap list on its welcome card. The modal's job — display
the manifest's caps to the user, then let them approve — is done
inline across the whole selection instead of serially per extension.

### Capability normalisation

Every recorded grant is funneled through
[`extensions::normalize_permissions`](../src/extensions.rs), which
drops anything not in `VALID_CAPABILITIES` with a warning. The Rust
list is authoritative; `CAPABILITIES` in
`ui/js/shared/extension-permissions.js` mirrors it for rendering the
install modal and settings badges.

This matters because it closes a small drift window: previously, JS
`normalizePermissions` filtered unknown caps when building the modal,
but the Rust commit path trusted whatever list the renderer passed
in. A typo in a bundled manifest (`"strage"`) could have ended up
recorded as a grant. It can no longer — both paths dedupe, lowercase,
and filter against the canonical list.

### Grant persistence

Grants are stored in config under `extension_grants[<id>]`:

```json
{
  "extension_grants": {
    "todos": {
      "granted": ["storage", "automation"],
      "approved_version": "1.0.0",
      "approved_at": "2026-04-26T12:34:56Z"
    }
  }
}
```

The `approved_version` field lets the runtime notice when an
extension has been updated. If the new manifest requests more
capabilities than the grant covers, the runtime silently drops the
extras until the user re-approves — the extension loads but those
invocations fail with a clear capability-missing error.

Uninstalling an extension clears its grant.

### Capability reference

See [`EXTENSIONS.md`](./EXTENSIONS.md) for the authoritative list
with per-capability descriptions. Summary:

`storage` · `clipboard` · `shell` · `filesystem` · `window` ·
`windows` · `notifications` · `calendar` · `session` · `agent` ·
`activity` · `automation` · `tts`

## Content Security Policy (CSP)

The Tauri webview runs with `"csp": null` (CSP disabled). The
security boundary is the extension sandbox and capability grant,
not webview CSP.

### Why CSP is disabled

Kage's first-party UI does not load external web content. All
content sources are:

- **Agent responses** — from the trusted ACP backend (kage-cli),
  rendered as markdown.
- **Local UI** — HTML/JS/CSS bundled into the binary.
- **Extensions** — loaded into sandboxed iframes subject to the
  permission system above.
- **Agent-produced JS** — run in a controlled eval context.

A strict CSP would block legitimate features (inline styles for
theming, eval for agent-produced JS, vendor libraries) without
adding protection given the sandbox is doing the heavy lifting.

### When to revisit CSP

- If the first-party webview ever starts loading external URLs.
- If untrusted third-party content is rendered in the main webview
  rather than an iframe.
- A stricter CSP on just the sandbox iframe's host document could
  be added as belt-and-braces without much downside.

## Defense in depth

Even with the sandbox doing most of the work, these additional
mitigations are in place:

- Markdown rendering sanitizes HTML document markers (`<!`,
  `<html>`) to prevent raw-HTML passthrough.
- Tool permissions require explicit user approval before the
  agent executes any ACP tool call.
- Tool permission events (grants, denials, revokes, expiries, and
  terminator-mode toggles) are recorded to
  `<config_dir>/kage/permission-audit.jsonl`. The log is
  **deliberately not tamper-evident** — it lives under the user's
  config directory with normal write permissions. Use it to
  spot-check recent activity, not for forensic audit. The settings
  page exposes a viewer and a "clear log" action.
- The computer-control MCP server is opt-in.
- Zip extraction defends against Zip Slip attacks, including
  symlink-based variants.
- Panic hook writes `crash.log` with backtrace for post-incident
  review.
- Mutex poisoning is recovered gracefully (`lock_or_recover`)
  so one thread's panic doesn't cascade.

## Summary of current guarantees

| Threat                                                | Status | Notes |
|-------------------------------------------------------|--------|-------|
| Extension calls an IPC command it didn't declare      | ✅ Blocked | Host-side capability check at the bridge. |
| Extension calls a "never allowed" command             | ✅ Blocked | Forbidden list in `extension-permissions.js`. |
| Extension reaches `window.__TAURI__` to bypass check  | ✅ Blocked | Iframe has null origin, no Tauri global. |
| Extension reads files outside its directory           | ✅ Blocked | `read_extension_file` is path-bounded. |
| New Tauri command silently exposed to extensions      | ✅ Blocked | Missing from table = blocked. |
| Extension installs without user approving its caps    | ✅ Blocked | Install is two-phase; no grant = no load. First-run welcome uses batch approval with caps shown on-card before Finish. |
| Renderer inflates caps beyond what a manifest declares| ✅ Blocked | Welcome path reads caps from on-disk manifest, not renderer. Store path validates user-approved list against canonical capability set. |
| Typo or unknown capability in manifest persisted as grant | ✅ Blocked | All grants go through `normalize_permissions` against `VALID_CAPABILITIES`. |
| Extension updates silently expand capabilities        | ✅ Blocked | Extras dropped until user re-approves. |
| Zip Slip during extension install                     | ✅ Blocked | Canonical-path + symlink checks. |
| Malicious markdown injecting script tags              | ✅ Blocked | Rendering pipeline strips document markers. |
| DOM-touching extension misuses settings-module access | ⚠ Trusted path | Tracked for migration. See "Which contribution points are sandboxed". |

## Known gaps being worked on

All first-class extension contribution points now run inside the
sandbox. Host HTML that extensions emit (widgets, custom result
rendering, message formatters, settings info blocks) is sanitized
through a narrow allow-list (see
`ui/js/shared/extension-html-sanitizer.js`). Action buttons route
back to the sandbox through declared RPC names rather than live
closures.

Future improvements being considered, none blocking:

- **Content-Security-Policy header on the sandbox iframe itself**,
  belt-and-braces beyond the sanitizer.
- **Per-widget refresh budgets** so a misbehaving widget can't
  thrash the CPU. Today widgets declare their own interval and
  the host trusts it within a lower bound.
- **Fine-grained capability grants** — let users approve a subset
  of the requested capabilities instead of all-or-nothing.

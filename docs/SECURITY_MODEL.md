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
| View                   | ✅ Yes — a visible sandboxed iframe in its own window; CSP blocks the view's own network access (the extension's hidden instances still have it, see "Known gaps"); the host bar and title are Kage's. See "Extension views". |

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
  "permissions": ["storage", "urls"]
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

`storage` · `clipboard` · `urls` · `launch` · `network` · `oauth` ·
`filesystem` · `window` · `windows` · `notifications` · `calendar` ·
`session` · `agent` · `activity` · `automation` · `tts` · `views`

`views` maps to no command. It lets the host open the windows an
extension's manifest declares (see "Extension views"), and Rust checks
it again on every open and every load of a view window.

`clipboard` is read access. Writing needs no capability but only ever
happens on a user's click, through the host: a search result's `copy`
action, a settings `action` with `copy`, or `context.view.copyText` in
a view window. The settings path accepts plain text only — at most 2048
characters, with no control characters other than tab and newlines, and
no bidi overrides, so a "Copy" button can't put terminal escape
sequences, or text that displays differently, on the clipboard. A view's
copy follows the same rules with a limit of 8192 characters, and only
while the view window is in front, right after a click or key press in
it, at most twice a second. `ext_view_copy_text` checks the text, the
focus and the rate again in Rust.

`oauth` listeners bind `127.0.0.1` on an ephemeral port and serve one
matching request. They close on timeout (at most 10 minutes), on
`oauth_loopback_cancel` (including while an await is in flight), and
when the sandbox that started them is torn down, which the host
handles by cancelling them. So a sign-in the extension cancelled, or
can no longer finish, can't be completed later in the browser. A
webview that dies without tearing its sandboxes down (a crash or
reload) leaves its listeners running until they time out.

## Content Security Policy (CSP)

The Tauri webview runs with `"csp": null` (CSP disabled). The
security boundary is the extension sandbox and capability grant,
not webview CSP.

### Why CSP is disabled

Kage's first-party UI does not load external web content. All
content sources are:

- **Agent responses** — from the trusted ACP backend (e.g. kiro-cli),
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

The one CSP Kage sets today is on an extension view's iframe (see
"Extension views"): it takes away that iframe's own network access.
The hidden sandbox iframes in the launcher, chat and Settings have none
yet (see "Known gaps").

## Links and navigation

A Kage window must never *become* a web page: it has full
`window.__TAURI__` access and no back button. (A plain
`<a href="https://developer.spotify.com/dashboard">` in the Spotify
extension's settings text once replaced the whole Settings window with
Spotify's site, because that window had no link handler.) Two layers:

1. **JS link handler** (`ui/js/shared/link-handler.js`), installed in
   every window — `ui-tests/shared/link-handler.test.js` fails if a
   window in `capabilities/default.json` doesn't call it. It listens on
   `window` in the capture phase, ahead of every page listener, so an
   ancestor's `stopPropagation()` can't hide a click from it. It cancels
   every anchor click (left and middle) and opens only `http(s)` /
   `mailto` through `open_url`; `kage:` routes internally; everything
   else (`javascript:`, `file:`, custom app schemes) is dropped. Two
   exceptions: a script-built `<a download href="blob:…">` (a settings
   "Export") is let through so the file is saved, and an anchor a
   first-party handler owns (`data-action`, `data-ext-action`) is
   cancelled but not opened, because that handler acts on it. It also
   strips `target` so `tauri-plugin-shell`'s injected `target=_blank`
   opener can't open the link a second time. Host code that renders
   extension HTML (formatters, widgets, settings info blocks)
   additionally runs `neutralizeLinks`, so no live `href` is left for
   the engine to follow.
2. **Rust navigation guard** (`src/navigation_guard.rs`). A Tauri
   plugin `on_navigation` hook — the only hook that also reaches the
   `tauri.conf.json` windows — cancels any navigation that isn't the
   app origin (`tauri://localhost`, `http(s)://tauri.localhost`, the dev
   server in dev builds) or an inert in-process document (`about:` for
   iframe bootstraps, `blob:`, `data:`). On macOS/Linux that covers
   iframes too; on Windows only main-frame navigations reach the hook.
   New-window requests are always
   denied (wry's default with no handler; on-demand windows install one
   that also forwards `http(s)`/`mailto` to the browser — only
   first-party top-level documents can raise them, since our iframes
   lack `allow-popups`).

**Who may open the browser.** A cancelled `http(s)` navigation is
handed to the browser **only on Windows**, where WebView2's
`NavigationStarting` is main-frame-only. WKWebView and WebKitGTK report
iframe navigations through the same callback without saying which frame,
and an extension's sandbox iframe can `location.href = …` at any time
with no user gesture — opening those would let an extension without
`urls` open arbitrary sites. So on macOS/Linux the guard only cancels,
and opening is the JS handler's job.

A user's click on a link in host-rendered extension HTML opens it
without the extension holding `urls`: the sandbox can't synthesize
clicks in the host document, the sanitizers limit link targets to
`http(s)`/`mailto` (narrower than `urls`, nothing that reaches
`launch`), and the user chose to follow a link they could see.
Programmatic opens (`context.invoke('open_url', …)`) still require
`urls`. So does an `open_url` a search result returns from `execute()`
(and `open_path` needs `launch`): the host runs the command for the
extension only after the same capability and argument checks as its own
`invoke()` (`ExtensionManager.extensionInvokeDenial`), and opens only
`http(s)` / `mailto` links that way.

**Links in a view need `urls`.** In an extension view the extension
owns the visible document and can fake a click, so there is no free
user-click path there. The view runtime cancels every link click
(capture phase) and opens absolute `http(s)` / `mailto` links through
`invoke('open_url')`, which the host checks like any other `invoke()`;
without `urls` nothing opens. A view iframe that navigates itself
(`location.href = …`) is cancelled by the Rust guard on macOS and Linux.
On Windows the guard doesn't see iframe navigations, so the view page
watches the iframe's `load` event on every platform: any load after the
first document means the frame navigated, and the page destroys the
sandbox at once and says so.

## Extension views

An extension with the `views` capability can show windows it draws
itself (`docs/EXTENSIONS.md` § Views; design:
`docs/design/EXTENSION_VIEWS.md`). Until views, no extension code
painted straight to the user or got raw keystrokes: everything else is
HTML the host sanitises. So `views` is its own capability, shown at
install time. The page in a view window (`ui/extension-view.html`,
label `ext-view:<extension>:<view>`) is first-party. The extension's
code runs in a visible iframe inside it, with the same sandbox and the
same capability checks at the port as every other contribution point.

- **The sandbox is the same.** The iframe's `sandbox` is exactly
  `allow-scripts`: a null origin, no Tauri, and no pop-ups, forms,
  modals, top navigation, pointer lock or downloads. Its `allow`
  attribute denies camera, microphone, geolocation, clipboard read and
  write, display capture, full screen, payment, USB, serial, HID,
  Bluetooth, MIDI, wake lock, web share and autoplay. The page itself
  gets its Tauri permissions from its own capability file,
  `capabilities/extension-view.json` (window pattern `ext-view:*`),
  which lists only the core event and window permissions it uses.
- **Identity comes from the label.** Rust builds the label from
  validated ids and derives everything from it. `ext_view_boot` takes
  no id argument, and a view window can open only views of its own
  extension. Rust checks the opener before it looks anything up, so a
  view can't learn whether another extension is installed.
- **Only a user action opens a view.** An extension returns an
  `open_view` effect from a handler the host runs for a click or key
  press (a result row, a card or widget button, a toolbar button, a
  settings action), or calls `context.view.open` in a view right after
  a click or key press there. Nothing opens one at any other time, so a
  timer or an agent tool call can't. `ext_view_open` accepts only the
  openers `floating`, `main`, `chat-*`, `settings` and `ext-view:*`,
  and checks again that the extension is installed and on, holds `views`
  (requested ∩ granted), declares the view, and sends valid params (a
  JSON object, at most 4096 bytes and 4 levels deep, never put in a
  URL). `ext_view_boot` repeats the first four checks on every load.
- **The extension can't pass for Kage.** Kage draws a bar above the
  iframe, outside it, that always names the extension, and the window
  title always ends with the extension's name (the extension sets only
  a subtitle). Kage never asks for a password, a passphrase or a
  permission inside a view window; its prompts appear in the launcher,
  chat or Settings. View windows are normal decorated windows: never
  frameless, transparent, always-on-top, element-full-screen or smaller
  than their `minSize` (at least 320 × 240), and the extension doesn't
  choose where they appear.
- **No network of its own.** The view's srcdoc starts with a CSP:
  `default-src 'none'`; scripts and styles inline or `blob:`; images,
  fonts, media and `connect-src` only `blob:` and `data:`; no frames,
  objects, forms or `<base>`. Before the view module loads, the runtime
  deletes WebRTC and hides `contentWindow` / `contentDocument` of child
  frames (an `about:blank` child isn't a navigation, so `frame-src`
  doesn't stop it). `window[0]` still reaches such a frame, so this is
  defence in depth, not a boundary. The bigger residual: the
  extension's hidden instances share its storage and can still reach
  the network, so a view can hand them what it shows or what the user
  types (see "Known gaps").
- **Window-bound commands are refused in a view**
  (`VIEW_SURFACE_DENIED_COMMANDS` in `extension-permissions.js`):
  sending prompts, cancelling, steering, tool responses, the window's
  session, the floating window's size and opacity, chat geometry,
  `notify_frontend_ready`, and the background agent calls. A prompt
  sent from a view would route its permission requests to a window
  with no permission UI.
- **The view's requests to the host are narrow:** a subtitle, close,
  open another of its views, compose (fills the launcher's input, never
  sends), copy plain text, open its settings. The last four need the
  view window in front and a recent user activation
  (`navigator.userActivation`, which script can't fake; where the
  engine lacks it, focus plus the rate limit). All but close are
  rate-limited (1 or 2 a second; 10 for the subtitle), and text is
  plain and capped (2000 characters to compose, 8192 to copy). Rust
  checks the focus, the text and the rate again for compose and copy.
- **Files dropped on a view never reach it.** Tauri's native drop
  handler takes OS file drops before the page sees them, and the page
  ignores its events, so neither contents nor paths reach the
  extension.
- **Window spam is bounded.** One window per view, at most 4 views per
  extension, at most 4 view windows at once (2 of them hidden). A view
  hidden for 10 minutes is destroyed. Turning the extension off,
  uninstalling it, or a grant without `views` destroys its view windows
  at once. On Windows, a view window whose WebView2 process fails is
  destroyed, not reloaded, so a crashing view can't loop. A busy view
  can't block the native close button, and the watchdog offers Reload.
- **Storage change events carry key names only**, of the extension's
  own keys, to its own sandboxes, and only to those that hold
  `storage`. The writer's window label becomes a `self` flag; the
  sandbox never sees a label.
- **Window geometry stays on the computer.** Each view's size and
  position (`<local data>/kage/extension-views.json`, owner-only) is
  never in a backup and never synced.

## Defense in depth

Even with the sandbox doing most of the work, these additional
mitigations are in place:

- Markdown rendering sanitizes HTML document markers (`<!`,
  `<html>`) to prevent raw-HTML passthrough.
- Tool permissions require explicit user approval before the
  agent executes any ACP tool call.
- Tool permission events (grants, denials, revokes, expiries,
  terminator-mode toggles, commands the user ran themselves from a
  sandbox notice, and every step of a command an agent asked Kage to run
  outside its sandbox) are recorded to
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

## Commands you run yourself

Agents run shell commands inside a sandbox of their own (kiro-cli behind
a managed wrapper, Codex, Claude Code's sandbox runtime). Some legitimate
commands can't run there at all — `brew install` applies its own
`sandbox-exec` profile, which macOS refuses to nest — and Kage's tool
approvals can't lift that sandbox. When a shell block's output reads as
the agent's sandbox refusing the command (`shellSandboxDenial` in
`ui/js/shared/shell-commands.js`), the block gets a notice that offers
to run it outside the sandbox (`shell-run-view.js`,
`src/commands/system/user_shell.rs`).

The trust boundary:

- **The notice is a hint, not a verdict.** It is triggered by text in
  the command's output, and any command can print that text itself. It
  never vouches for the command; it shows the whole command, with every
  hidden or control character marked, and asks the user to read it.
- **The user reads and approves every run.** "Run it here" and "Run in
  Terminal" only open a confirm inside the notice: the exact text that
  will run (monospace, scrollable), the directory it runs in (the tool
  call's `cwd` when the agent sent one, else the home folder — checked
  to exist before the confirm offers to run), a warning that it runs
  outside the agent's sandbox with the user's full permissions, and a
  caution line per risky pattern (`sudo`, `rm -rf`, `curl … | sh`,
  `chmod/chown -R` on `/`, a top-level folder or home, writes to shell
  startup files). A command taller than its box gets a line saying
  so, since an overlay scrollbar may not show there's more. Only the
  confirm's Run button starts a run, once, on a click of its own (the
  rest of a double-click that opened the confirm is ignored); focus
  starts on Cancel. There is no "always allow" and nothing is
  remembered: the next run needs its own confirm. A command with hidden
  characters gets no run button at all (only Copy).
- **Kage never runs agent commands on its own.** Nothing the agent sends
  — output, tool calls, markdown — becomes a click: it's rendered as
  text, and the run path starts only from the confirm's click handler.
- **Only Kage's own windows can call it.** `run_user_shell_command`,
  `cancel_user_shell_command`, `user_shell_run_context` and
  `run_command_in_terminal` refuse any window but `main`, `chat-<uuid>`
  and `floating`, and are mapped to `null` in `extension-permissions.js`,
  so the sandbox host refuses them for every extension whatever it was
  granted. (Extension iframes have no Tauri IPC of their own: no
  `window.__TAURI__`, no invoke key.)
- **How it runs.** The exact string, unchanged, through the user's login
  shell — `$SHELL -l -c -- <command>` when `$SHELL` is a POSIX shell, else
  `/bin/zsh`, else `/bin/sh` (the `--` keeps a command that starts with
  `-` or `+` from being read as shell options); on Windows PowerShell (`-NoProfile -NonInteractive
  -Command`), or `cmd /D /S /C` when the agent's tool was `cmd`
  (kiro-cli's `execute_cmd`) — with the login-shell environment overlay,
  stdin closed, in its own session and process group (no terminal, so
  `sudo` fails at once instead of prompting where nobody sees it; the
  confirm offers Terminal for those). It is stopped (the whole process
  group) by Stop, after 10 minutes, when its window closes or its page
  reloads (nothing would be left to show it or stop it), and when Kage
  quits. One run per window at a time. Run ids are random, because
  Tauri delivers the output events to every Kage window that listens.
- **What's kept.** The calling window receives the output as it streams
  and the last 64 KiB with the exit status. If the user left "Tell the
  agent the result" on, Kage sends the agent the command, exit status and
  the last 8 KB of output as the user's next prompt, visible in the
  chat — waiting for a turn in progress to end, and droppable until
  then. A run the user stopped is not reported unless they ask.
- **What's recorded.** An English line in the app log (window, program
  name, length, exit status — never the command line), a `user_ran_command`
  entry in the permission audit log (the command clipped to 2 KB, its
  directory, outcome and exit code), and, with analytics on, a
  `user_shell_run` event carrying only where it ran and an exit-code
  bucket.
- **Run in Terminal (macOS).** Hands the command to a new Terminal.app
  window over Apple Events (`osascript` with a constant script; the
  directory and command travel as arguments, never spliced into script
  source), where it runs in the user's interactive shell with a real
  TTY, so `sudo` can ask for a password. Terminal types the command
  into that shell as keystrokes, so a command with a tab (completion)
  or any control character other than a newline is only copied; the
  backend refuses it too. The first time, macOS asks
  whether Kage may control Terminal. Elsewhere there's no safe
  equivalent (Windows Terminal re-parses `;`; Linux terminals each quote
  differently), so the notice only copies the command.

## Commands an agent runs outside its sandbox

The agent can also ask for this itself. Kage's own MCP sidecar (the
`kage-computer-control` server Kage registers in the agent's
`mcp.json`, opt-in like the rest of computer control) offers a
`run_outside_sandbox` tool: the command, an optional working directory,
the agent's reason (why its own shell can't run it) and an optional time
limit (default 600 s, at most 1800 s). Its description tells the agent to
use it only after its own shell failed because of its sandbox, never for
credentials or secrets, and that the user approves every run unless they
made a rule. Code: `computer_control_mcp/src/run_outside_sandbox.rs`,
`kage-core/src/host_bridge.rs` (protocol), `kage-core/src/os/host_bridge.rs`
(transport), `src/host_commands/` (decisions), and
`ui/js/shared/host-command-view.js` (prompt).

The trust boundary:

- **The sidecar can't run anything itself.** It is a child of the
  sandboxed agent, so it is sandboxed too; it only hands the request to
  the Kage app, which isn't, over the host-command bridge and waits for
  the reply. If the agent's sandbox blocks the bridge, the tool says it
  couldn't reach Kage and nothing runs.
- **The bridge is local and authenticated by the OS.** No TCP, no
  listening network port, no token. On macOS and Linux it is an AF_UNIX
  stream socket, `host.sock`, in `<local data dir>/kage/bridge/` (for
  example `~/Library/Application Support/kage/bridge/`). Kage creates that
  directory `0700` and refuses to listen when it — or the socket path —
  is a symlink, isn't owned by the user, or allows group or others
  anything, or when its parent is writable by others or owned by anyone
  but the user or root; a leftover socket is removed only when it's the
  user's and nothing listens on it. Every
  connection's peer uid (`getpeereid` on macOS, `SO_PEERCRED` on Linux)
  must equal Kage's effective uid, else it is dropped unread; the
  sidecar checks the server's uid the same way. On Windows it is a named
  pipe, `\\.\pipe\kage-host-bridge-<user SID>`, created as the first
  instance (`FILE_FLAG_FIRST_PIPE_INSTANCE`), refusing remote clients
  (`PIPE_REJECT_REMOTE_CLIENTS`), with a DACL granting only the user and
  SYSTEM; the client process's token user SID must equal Kage's, and the
  sidecar checks the server's. Kage creates the next pipe instance before
  handing a connected one out (or closing one whose client already left),
  so the name is never free to claim. Both
  sides compute the endpoint the same way (`default_endpoint`), so
  nothing is passed through the agent's `mcp.json`; isolated user dirs
  (tests, `KAGE_USER_DIRS`) move the socket with the data dir and add a
  hash of the isolation root to the pipe's name, so a test never reaches
  the user's running Kage. "Same user" is the
  boundary: any process of the user can reach the bridge — which can't
  get it more than a prompt, below.
- **The protocol is small and strict.** One request per connection: one
  JSON line with a version field, exactly the documented fields (unknown,
  duplicate or mistyped ones are refused), at most 64 KiB (command
  32 KiB, reason 2 KiB, working directory 4 KiB). One reply line: the
  outcome, exit code, duration and the output's last 32 KiB — or
  "denied", "timed out waiting for approval", or an error. A client has
  five seconds in all to send its request line (however slowly it
  trickles in; on Windows the pipe read polls for data so the same limit
  holds), else it is dropped. At most 16 connections are handled at once
  and 8 prompts wait at once.
- **Kage matches the request to the session that asked.** The
  notification handler remembers the agent's in-flight
  `run_outside_sandbox` tool calls (MCP calls only: a call whose kind is
  a built-in tool's, such as a shell command titled like this tool, is
  never one); a request is matched to the newest open call with exactly
  its command and working directory (`rawInput.command`, `rawInput.cwd`),
  which names the ACP session and so the window to ask in. The folder is
  part of the match because a match is what lets rules apply: a request
  from elsewhere naming a visible call's command can't get that call's
  standing in a folder of its own choosing. A request Kage can't match —
  an agent outside Kage, or any other process of the user — is shown in
  the floating window with a system notification naming the program that
  asked, as the sidecar reports it.
- **The agent's tool approval is the execution prompt.** When the agent
  asks Kage to approve the MCP call itself (`Running:
  @kage-computer-control/run_outside_sandbox`,
  `mcp__kage-computer-control__run_outside_sandbox`, MCP kind, every title
  naming exactly that server and tool), Kage allows it without a prompt:
  the real decision happens when the command reaches Kage, so the user
  isn't asked twice. A per-tool Deny the user set still wins.
- **The user decides, in the window that owns the session.** The prompt
  is inline (in the answer being written, or in the tray above the
  input) and reuses the "Run it here" confirm: the whole command with
  hidden characters marked, where it runs and in which shell, the agent's
  reason (as text), the full-permissions warning, the risk lines, and
  which agent asked. Its answers are Run once, Always allow commands
  starting with [editable words, default the first two], and Deny. Each
  prompt answers only its own request, only from the window it was shown
  in (the backend checks), once. Nothing is focused when a prompt
  appears, so keys typed elsewhere can't answer it; Run and Always allow
  take a single click of their own (or Space), never Enter — not in the
  words field, and not on a button Tab or Shift+Tab from the composer
  landed on. With no answer in five minutes the request is denied; if
  the agent stops waiting first — it closes the connection, the user
  stops its turn (`cancel_generation`), or its tool call ends (failed,
  timed out on the agent's side) — the prompt is withdrawn and nothing
  runs. A command with hidden characters is
  never run from Kage: its prompt offers Copy and Deny only.
- **Rules are narrow.** "Always allow commands starting with …" saves a
  rule (`config.tool_permissions.host_command_rules`) that covers only a
  *simple* command — printable ASCII, no `; & |`, no redirection, no
  `$`, backticks or `( )`, no escapes, globs, braces, `!`, `#`, `^` (zsh's
  extended-glob negation) or `%` (Windows `%VAR%`), no line breaks, and no
  `NAME=value` before the program — whose first words are
  exactly the rule's (`brew install` covers `brew install awscli`, not
  `brew installer`, `sudo brew install` or `brew install x; rm …`). When in
  doubt, Kage asks. "Run every command without asking"
  (`host_commands_allow_all`, Settings only, behind a warning that
  commands then run with full access to the computer, including AWS
  credentials) skips the prompt for everything else. Neither is implied
  by "Trust all", terminator mode, trusted kinds or trusted MCP servers;
  both are backend-owned (a Settings snapshot, a backup or sync never
  changes them) and machine-local. They apply only to a request matched
  to a session the user sees: an unmatched request, or one from a
  background session (inline assist, macros — text from other apps),
  always asks.
- **The credential guard always asks.** Whatever the rules or switch
  say, a command that names an AWS, SSH or cloud credential store or the
  keychain (`~/.aws`, `$HOME/.aws`, `AWS_SHARED_CREDENTIALS_FILE`,
  `.sbox-session`, `~/.ssh`, `id_rsa` / `id_ed25519`, `.netrc`, `gh`'s
  `hosts.yml`, gcloud credentials, Azure / Kubernetes / Docker credential
  files, `~/Library/Keychains`), asks a CLI for credentials (`security
  find-*-password`, `dump-keychain`, `aws configure export-credentials`,
  `aws sts get-session-token`, `gh auth token`, `gcloud auth
  print-access-token`, `aws secretsmanager get-secret-value`, …), names a
  credential environment variable, dumps the whole environment (`env`,
  `printenv`, `set`, `export -p`, `ps e`, PowerShell's `env:` drive, …)
  or globs into hidden folders — or runs in a folder inside one of those
  stores (the working directory as given and as it resolves, so a link
  into `~/.aws` counts) — prompts with a line saying why
  (`src/host_commands/guard.rs`). A working directory with hidden or
  control characters is refused outright: the user would approve a
  different folder from the one the command runs in. It is best-effort: it reads the
  command's text, so a script, an alias, a program that reads those
  files itself or an obfuscated path gets past it — which is why rules
  cover only simple commands and the user still reads what runs. It
  matches the purpose of the agent's credential sandbox: preventing
  credentials from being used by accident, not stopping a determined
  local program (which, running as the user, could read them anyway).
- **How it runs.** Exactly as "Run it here" runs a command (the same
  runner: login shell, login-shell environment, stdin closed, own
  process group, output capped), with the request's time limit. The run
  shows in the prompt's place with its live output, time and Stop; it is
  stopped by Stop, at its time limit, when its window closes or reloads,
  when the agent stops waiting, and when Kage quits. At most four agent
  runs go at once. The result goes back to the agent as the tool's
  result; no chat message is sent.
- **What's recorded.** The permission audit log gets a `host_command`
  entry per step — `requested`, `guard`, `auto_approved` (which rule, or
  allow-all), `approved` (once / always), `denied`, `expired`,
  `withdrawn`, `refused`, `finished` (outcome, exit code) — and for
  `rule_added`, `rule_removed`, `allow_all_on` and `allow_all_off`, with
  the command clipped to 2 KB. The app log gets English lines without
  the command line. With analytics on, `host_command` and
  `host_command_run` events carry counts and buckets only (which step,
  whether the guard fired, whether the request was matched, an exit-code
  bucket) — never the command or a path.

### The agent can't press Kage's buttons

The same sidecar's computer-control tools inject input into other apps.
They could otherwise press Kage's own Run, Always allow or tool-approval
buttons. Every input-injecting tool refuses, with an error telling the
agent to bring the app it means to the front, when its target is a Kage
window (the `kage` executable, or bundle id `com.kage.launcher`):
keyboard tools when Kage is the active app or holds keyboard focus (a
key panel included); mouse tools when their point — the cursor when none
is given, both ends of a drag — lies on a Kage window; element tools
when the element belongs to Kage (on Windows, WebView2 content reports
`msedgewebview2.exe`, so a WebView2 process Kage started — its parent, or
its parent's parent, is Kage — counts as Kage, whether or not another
window covers it; failing that, the window under the element decides).
Read-only tools still work. This is best-effort too: the agent's own
shell could still script the UI (it runs inside its sandbox, behind the
tool-permission prompt).

## Backups and sensitive data

A backup (Settings → About → Backup & restore) is a file the user moves
between machines, so it is treated as something that can leak. There are
exactly two kinds (`src/config_export.rs`), and no unencrypted full backup:

- **Everything, encrypted** (the default): config, steering docs,
  extension settings and extension data, AES-256-GCM-encrypted with a key
  derived (Argon2id) from a passphrase. An empty or all-whitespace
  passphrase is refused.
- **Settings only, not encrypted**: an **allowlist**, not a scan. Every
  field of `Config` has an explicit decision in
  `src/config_export/allowlist.rs` (`FIELDS`): copied, with the shape its
  value must have, or left out and why. The file is built by walking that
  table, never the config, so a field nobody decided on is left out — and
  the `every_config_field_has_a_decision` test fails until someone
  decides. Copied values are booleans, numbers, closed sets (time and date
  formats, update channel, power mode, window position, built-in voices),
  a built-in theme or an installed theme's id, a language Kage ships,
  hotkeys the runtime's parser registers (validated whole with the same
  parser, modifier aliases written under the name the picker uses), and
  the ids, capabilities, approved versions and timestamps of installed
  items; nothing free-form. Every number must be finite, of its field's
  type and within that setting's range (the Settings control's, or for
  one without a control what the code can use: `allowlist::Range`); a
  value outside is rejected, never clamped — left out of the file and
  named, and on import this machine's value stays. As defence in depth, a copied string that
  still looks like a password, token or key (`looks_like_secret` in
  `src/config_export/sensitive.rs`) is left out too, and the UI names it.
  Left out entirely: steering docs, app modes, prompts, shortcuts and
  automations (scripts, arguments, URLs), agent connections, internal
  sites, tool-permission rules and trust, store addresses, paths, the
  install id, diagnostics switches, caches, and every extension's data.
  Extension settings are copied only where **every** installed extension
  folder for the id declares the field, no item claiming the id (a theme,
  a second folder) marks it `sensitive` or refuses the value — those can
  only make a setting stricter — and the value is a boolean, a number, one
  of a declared `enum`, or a string the manifest marks
  `config.<field>.backup: "plain"`; never a value of any type — boolean,
  number and `enum` choice included — under a credential-like name (the
  `SENSITIVE_NAME_PATTERN` safety net, one of the extension's
  `sensitiveData` keys, or a field any manifest marks `sensitive`): that
  check runs before any declared shape is looked at, and every string
  that's copied, a declared choice included, also goes through
  `looks_like_secret`. An extension that isn't installed has no
  manifest to ask, so none of its settings are copied. Before the file
  is saved, the UI lists what it leaves out from this machine
  (`preview_config_export`: labels and counts, never values).

`export_config_bundle` enforces both in the backend: `mode: "encrypted"`
without a usable passphrase is refused, and `mode: "settings_only"` never
takes one. Scratch (`__`) extension settings and the tool-permission
history the agent records on its own ("ask" rules) are never in any
backup. All four backup commands (`export_config_default_filename`,
`preview_config_export`, `export_config_bundle`, `import_config_bundle`)
are off-limits to extensions.

Bundle formats: v1 is a full backup (what older builds read); v2 is what
earlier builds wrote when a backup left items out or still carried their
placeholder `__REMOVED_FROM_BACKUP__` (this build writes it for a full
backup still holding such placeholders, so a build that would write them
over real values refuses it); v3 is settings-only, which older builds
refuse rather than import as a full backup.

**Import** (`src/config_export/merge.rs`) keeps this machine's value
wherever the backup's is missing, empty, or an earlier build's
placeholder — bound to identity, so a local secret is never moved to a
different item or place: Kage's singleton settings are the same setting
(a string holding a placeholder is restored only if the rest of it
matches, so `https://evil/…?t=<placeholder>` never receives this machine's
`https://store/…?t=…`); agent connections and internal sites match by a
unique `id` and the same connection type, and a site only when every
other field (URL and content rules, sample address, enabled, language,
timeout) matches; other lists only as exact twins. One rule, in every
kind of import, keeps a local credential where it is: an extension whose
credential stays on this machine — a sensitive setting, or a data file
holding one (declared in `sensitiveData`, or named like a credential, or
JSON with a credential-named value), that the bundle doesn't replace with
its own, judged file by file; or any entry in its data folder Kage can't
read (a file it may not open, a link it never follows), whatever the
bundle carries — keeps this machine's settings and data files whole,
takes none of the bundle's, and the summary names it. No field of the
bundle's is compared with this machine's to decide where a credential
goes, so nothing in the bundle — whatever it's called, a number or a URL
path — can point that credential somewhere else. Other extensions'
settings merge by name, recursively, a missing or empty value keeping
this machine's: none of them holds a credential of this machine's.
Settings are an object: anything else in the bundle's place keeps this
machine's whole. A hotkey's key and modifiers are merged together. A setting or data file an extension
declares sensitive (or named like a credential) is never replaced by an
empty or missing one, in any bundle version; an extension-data file whose
copy holds nothing never replaces one that does. In an earlier build's
"leave out" bundle, which never carried a sensitive value, a sensitive
value or file there was put in by hand and never replaces this machine's;
a bundle carrying a file its manifest says it left out is refused. A
settings-only bundle merges only allowlisted fields (each re-checked
against its shape and this machine's manifests; for an extension not
installed here only booleans and numbers) and leaves everything else
untouched. It never carries a credential, so it changes no setting of an
extension that holds one here (in its settings or its data files, or a
data file Kage can't read), and names it in the summary; it takes
at most 512 ids per map and 128 settings per extension, and leaves a
quarter of config.json's size limit free; one carrying steering or
extension data is refused. An
import whose merged settings would be larger than `Config::load` reads
(1 MB) is refused,
and so is any import while this session can't save config.json (it came
from a newer Kage and is being preserved).
Tool-permission rules are merged (a backup adds and updates rules but
never removes one of this machine's); category-wide tool trust stays this
machine's. Extension capability grants are never taken from any backup,
of any kind or version, encrypted or not (`src/config_export/consent.rs`):
the merged `extension_grants` is this machine's, whole. A backup may
switch an extension on only when this machine's grant already covers
everything its installed manifest requests; an installed extension the
backup has on, or claims a grant for, that this machine's grant doesn't
cover is switched off and listed in the summary as needing approval. The
Store's **Approve** button for such an extension runs the install-time
permission prompt again (`commit_extension_install` records the grant).

`config.json` from a backup goes through `config_migrations` the way
`Config::load` does before anything is merged or written: a schema from a
newer Kage is refused ("update Kage"), and the merged config is saved at
this build's schema.

Import only writes to this machine's steering docs (at this machine's
paths: a bundle's `user_steering_path` is ignored and the local one
kept) and under `extension-data/`. Archive paths are plain `/`-separated
names on every platform (no `..`, `\`, `:`, `~`, `<>|?*"`, NUL, device
names). An incoming name that differs from a local one only by case or
Unicode normalisation is treated as that file when this file system
resolves both to the same file (APFS, NTFS; not a case-sensitive disk);
no symlink under the root is followed (each component is `lstat`ed). The
file is capped before it is read (`MAX_FILE_BYTES`, by its size and a
capped reader), the encrypted envelope before key derivation, and the
archive before it's unpacked, and decryption happens in place. Entries are
read only after their names are checked, and capped (10,000 entries,
64 MB each, 256 MB in all, measured as they inflate; entries sharing data
are refused).

Import is all or nothing and survives a crash: every destination is
checked, every file is staged to a temp file next to its target, then a
durable journal (`import-journal.json`, owner-only,
`src/config_export/journal.rs`) records every target, its staged file,
the name its current copy is moved aside to, and a copy of `config.json`;
only then do renames commit the files. The caller saves `config.json`
under the same config lock as the merge (so a concurrent save can't be
lost) through the transaction, which records in the journal what it
writes (SHA-256 and length before writing it, and on Unix its inode once
it's there), then finishes the transaction, which records the commit
marker before deleting the old copies. A failure puts everything back. An import
interrupted by a crash is resolved before the config is read on the next
start (or by the next import): committed → its leftovers are deleted;
not committed → every file and `config.json` are put back, so files and
config always match. Undoing never removes or replaces a file the
import didn't write: the journal records what each write puts at its
target (SHA-256, length and, on Unix, device and inode, which the rename
keeps), and a target that has changed since (an extension wrote it
again, or a new file is there) is kept, with this machine's earlier copy
beside it. Each finished undo step (a file put back, `config.json`
restored) is appended to the journal and flushed before the next one
starts, so a retry skips what's done and never touches a file created
since. `config.json` is undone the same way: its earlier copy is put back,
or the file removed when there was none, only while it's still the one
the import saved; one saved since (after a recovery that stopped partway,
say) is kept, the earlier copy beside it. A rollback copy is deleted only once
its transaction committed; one whose files can't all be reached or put
back keeps the journal, and every copy, for the next try (after five failed tries the
journal is set aside under another name, copies still kept, so imports
aren't blocked for good). An import undone by the running Kage leaves no
config snapshot for a later start to restore over newer saves, and a
journal is never written over. Imports hold an OS file lock (`.kage-import.lock`), so a
second Kage process never resolves one still running. Extension data
and the auto steering doc are written owner-only, whatever they had
before; the user's own steering doc keeps its permissions (owner-only
when new). Replacing is a rename,
so another hard link to a replaced file keeps the old content. An
encrypted backup is recognised by its first bytes, whatever it is called.

## Folder sync

Folder sync (docs/design/SYNC.md) keeps computers in step through a
folder a cloud app already syncs. Anyone who can read or write that
folder (the cloud provider, a shared link, a leaked cloud account) is
outside the trust boundary. The folder layer (`src/sync/envelope.rs`,
`crypto.rs`, `folder.rs`, `fsio.rs`, `keystore.rs`) enforces:

- **Everything Kage writes there is encrypted and authenticated.** Each
  `.kage` file is AES-256-GCM with a random nonce. The authenticated data
  is `kage-sync|1|<folder id>|<device id>|<kind>|<file name>`, so a
  planted or edited file fails to open, and so does one copied into
  another computer's directory, renamed, or moved to another folder.
  Shards and blobs are named by a keyed hash of their content, and the
  reader checks the name after decrypting. Plaintext is compressed before
  it is encrypted; no credential is ever in it.
- **Opening is bounded.** A file over its kind's limit is refused before
  it is decrypted, and decompression stops at the plaintext limit (64 KiB
  for a head, 4 MiB for a shard, 16 MiB for a blob), so a small file
  can't inflate into a large one.
- **The data key never leaves a computer in the clear.** A random 32-byte
  key per epoch is wrapped in a key file by Argon2id of the sync
  passphrase (64 MiB, t = 3; a key file asking for more than 256 MiB,
  t = 8 or p = 4 is refused, since anyone with folder access can plant
  one). Its AAD names the folder and epoch. The passphrase is never
  stored or logged. Subkeys come from HKDF-SHA256 with fixed labels.
- **The key store doesn't roam.** The OS keychain where it is reliable
  (macOS only with a team identifier in the build's signature; Windows
  Credential Manager with `CRED_PERSIST_LOCAL_MACHINE`; the Linux Secret
  Service), else an owner-only `key.bin` in the non-roaming local data
  folder. Isolated runs (tests, the mock-app sweep) only ever use memory.
  Key buffers are zeroized on drop.
- **Links aren't followed.** A symlink or junction inside the sync folder
  is never followed: a directory Kage made there that turns out to be a
  link fails the call, listings leave links out, and only regular files
  are read. Files are created with exclusive create or a temp-then-rename
  that refuses to replace anything.
- **A cloud driver can't hang Kage.** Every folder call runs on a pool of
  two threads with a timeout. A stuck call is abandoned; after four the
  pool stops taking calls and sync pauses, rather than growing threads.
- **Names are strict.** File names are classified by fixed patterns;
  anything else is never read and never deleted.

Credentials, tool permissions, capability grants and consent never sync,
not even encrypted (docs/design/SYNC.md §11.5).

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
| Link or redirect turns a Kage window into a remote page | ✅ Blocked | JS link handler in every window + Rust `on_navigation` guard. See "Links and navigation". |
| Extension opens the browser without `urls` via an iframe redirect | ✅ Blocked | The guard never opens URLs it can't attribute to the main frame. |
| An extension view shows a web page inside a Kage window | ✅ Blocked | Rust guard on macOS/Linux; the view page stops a frame that navigates on every platform. See "Links and navigation". |
| An extension view imitates Kage (a fake permission prompt, passphrase request or sign-in) | ⚠ Mitigated | Kage's bar and the window title always name the extension; Kage never prompts inside a view; views open only on a user action and can't be frameless, transparent or always-on-top. See "Extension views". |
| An extension view sends data out over the network | ⚠ Mitigated | The view's own CSP allows no remote requests and WebRTC is removed. The extension's hidden instances still have the network and share its storage. See "Known gaps". |
| An extension floods the screen with windows | ✅ Blocked | Opening needs a user action; one window per view, 4 views per extension, 4 view windows in all (2 hidden); rate limits inside a view. |
| An extension view calls a command bound to the launcher or chat window | ✅ Blocked | `VIEW_SURFACE_DENIED_COMMANDS`. |
| DOM-touching extension misuses settings-module access | ⚠ Trusted path | Tracked for migration. See "Which contribution points are sandboxed". |
| Agent output or an extension runs a shell command outside the agent's sandbox | ✅ Blocked | Only the confirm's own click runs one, once; the commands are `null` for extensions and refuse non-Kage windows. See "Commands you run yourself". |
| An agent runs a command outside its sandbox without the user | ✅ Blocked | Only the user's answer in Kage, or a rule they made for simple commands (never for credential-touching ones or ones with hidden characters), runs it. See "Commands an agent runs outside its sandbox". |
| Another user's process reaches the host-command bridge | ✅ Blocked | 0700 socket directory and peer-uid check (macOS/Linux); per-user pipe DACL, remote clients refused and token-user check (Windows). |
| The agent presses Kage's own approval buttons with computer-control tools | ⚠ Best-effort | Input tools refuse Kage's windows (by process); the agent's own shell could still script the UI. |
| Unencrypted backup file leaks a token, key, script, prompt or steering doc | ✅ Blocked | The only unencrypted backup is settings-only: an allowlist of shaped values, no free-form text, no extension data. Backend-enforced. See "Backups and sensitive data". |
| Restoring a backup wipes a sign-in this machine has   | ✅ Blocked | Missing, empty and placeholder values keep the local one, bound to the same item; an extension signed in here keeps its settings and data whole. |
| An interrupted import leaves files and config out of step | ✅ Blocked | Durable journal with a commit marker; resolved before the next start reads config. |
| Undoing an import deletes data written since          | ✅ Blocked | Each undo step is recorded before the next; a target, `config.json` included, is removed or replaced only while it's the file the import wrote (hash, length, inode). |
| A backup grants an extension capabilities             | ✅ Blocked | Grants are never imported, in any kind of backup; an extension the backup would switch on stays off until this machine's grant covers its manifest. |
| A backup points a credential kept here somewhere else | ✅ Blocked | In every kind of import, an extension whose credential stays here (or with a data file Kage can't read) keeps its settings and data whole; no field is compared to decide. |
| Someone with write access to the sync folder plants or edits a file | ✅ Blocked | AES-256-GCM bound to folder, computer, kind and name; content names checked after decrypting. See "Folder sync". |
| A planted link in the sync folder makes Kage write elsewhere | ✅ Blocked | Links inside the folder are never followed. |

## Known gaps being worked on

All first-class extension contribution points now run inside the
sandbox. Host HTML that extensions emit (widgets, custom result
rendering, message formatters, settings info blocks) is sanitized
through a narrow allow-list (see
`ui/js/shared/extension-html-sanitizer.js`). Action buttons route
back to the sandbox through declared RPC names rather than live
closures.

Future improvements being considered, none blocking:

- **Content-Security-Policy on the hidden sandbox iframes.** A view's
  iframe has one (see "Extension views"). The hidden iframes in the
  launcher, chat and Settings don't yet, because existing extensions
  may rely on direct requests; store extensions need an audit first.
  Until then, a hidden sandbox can make direct network requests to a
  server that allows any origin, whatever its grant. The same
  extension's hidden instances share its storage with its views, so a
  view's CSP is defence in depth: an extension can still send out what
  its view shows by writing it to storage for a hidden instance to
  send.
- **Fine-grained capability grants** — let users approve a subset
  of the requested capabilities instead of all-or-nothing.

### Widget refresh budgets (implemented)

Widgets declare their own refresh interval, but the host enforces
hard bounds and a circuit breaker:

- The declared interval is clamped to `[1s, 24h]` so a typo'd `1`
  can't spin the host on a 1ms loop.
- Widgets without a registered, connected host slot never start refreshing.
  Timers stop while the hosting window is hidden and resume on show, with an
  immediate refresh when content is missing or stale. Configuration updates
  re-read the declared interval and re-arm the timer.
- Re-entrant ticks are dropped — `setInterval` keeps firing even if
  the previous render is still in flight; we skip overlapping
  invocations rather than letting them stack.
- Each render is timed. A render that takes longer than 5 seconds
  *or* longer than 70% of the widget's own declared interval counts
  as a "slow render" failure.
- After 3 consecutive failures (slow renders, RPC throws, RPC
  timeouts, or skipped overlaps) the breaker trips: the timer is
  cleared, the host slot shows a "Widget paused" notice with a
  manual retry link, and an `extension_widget_disabled` telemetry
  event fires (extension id + reason, no widget content).
- A successful render resets the failure counter so transient blips
  don't add up forever.
- A manual retry resets the counter once. If the widget trips again
  after that, the user has to disable the extension to recover —
  intentionally avoiding an automatic-retry loop that would let a
  broken widget burn CPU indefinitely.

See `ExtensionManager._mountWidget` and `_renderWidget` in
`ui/js/shared/extension-manager.js` for the implementation.

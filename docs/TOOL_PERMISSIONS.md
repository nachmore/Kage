# Tool Permissions

How Kage decides whether to allow a tool call from the agent. The trust
boundary lives in the Rust backend; the frontend modal is just the prompt
that fills in the user's decision.

## What the user sees

When the agent requests a tool that isn't already allowed, a modal appears
with four options:

- **Once** — allow this single call.
- **24 hours** — auto-approve this tool for the next 24h, then ask again.
- **Always** — auto-approve indefinitely, with a 30-day staleness check.
- **Deny** — reject this call.

Per-tool policies are keyed by the tool call's title. An agent's shell
tool titles every call with the command itself (kiro-cli:
`Running: git status`), so for CLI commands **Always** only covers that
exact command string. For those prompts the Allow menu also offers:

- **Trust all commands…** — after an inline danger confirmation, trusts
  the whole ACP tool kind `execute` (`trusted_kinds`), approves the
  current request and any command prompts already queued in that window.
  Every later shell command is auto-approved. Note what that means: a
  command can do anything the user's account can (edit or delete files,
  use the network), so the prompts that remain only cover the agent's
  *other* tools. Prompts from background features (inline assist,
  macros, script generation — sessions whose tool calls are never shown)
  never use the trust and still ask. Toggle it in Settings → Tool
  Permissions ("Trust all CLI commands").

The same applies to MCP tools: the title names one tool of one server
(kiro-cli: `Running: @builder-mcp/InternalSearch`), so **Always** covers
only that tool. When the backend can identify the tool's MCP server (see
[MCP server identification](#mcp-server-identification)), the Allow menu
also offers:

- **Trust all tools from `<server>`…** — after an inline confirmation,
  trusts every tool of that server (`trusted_mcp_servers`) — the tools it
  has today and any it adds later, including ones that change or send
  data — approves the current request and any requests from the same
  server already queued in that window. A tool with an explicit **Always
  deny** stays denied, and background-feature prompts still ask (as for
  commands). Settings → Tool Permissions lists the trusted servers, each
  with **Stop trusting**; **Reset All Permissions** revokes them all.
  Trust is by name: a different server later registered under the same
  name is trusted too.

Three settings change this prompt flow:

- **Trust mode (`trust_all`)** — auto-approves every tool call without
  prompting, except those with an explicit `deny` policy.
- **Terminator mode (`terminator_mode`)** — same as `trust_all` but
  intended as a temporary "leave me alone" toggle. Toggling it on/off
  is recorded in the audit log.

- **Trusted tool kinds (`trusted_kinds`)** — category-wide trust; today
  only `"execute"` (CLI commands). An explicit per-tool `deny` still
  wins. Backend-owned: `save_config` ignores changes to it and a config
  import keeps this machine's value — it changes only through the
  audited `set_tool_kind_trusted` command.
- **Trusted MCP servers (`trusted_mcp_servers`)** — per-server trust;
  exact, case-sensitive names. An explicit per-tool `deny` still wins.
  Backend-owned in the same way: `save_config` ignores changes to it, a
  config import keeps this machine's value, and it changes only through
  the audited `set_mcp_server_trusted` command (which refuses names that
  aren't plain identifiers).

All default to off. All surface a warning in Settings when enabled.

## MCP server identification

ACP's `ToolCall` has no MCP server field, and no agent we've seen sends a
dedicated one, so `mcp_server_from_title` (`src/config/permissions.rs`)
reads it from the title — conservatively, refusing anything that could
split more than one way:

- kiro-cli: exactly `Running: @<server>/<tool>`.
- Claude Code: exactly `mcp__<server>__<tool>`, with one `__` separator and
  no run of three or more underscores. Server `acp` is refused: Claude
  Code's ACP adapter routes its built-in Edit/Bash/Read tools through an
  in-process MCP server of that name.

Server and tool must be plain identifiers (ASCII letters, digits, `-`,
`_`, `.`; server at most 64 characters), so spaces, extra slashes, `;`,
newlines or trailing text mean "not MCP". On top of that the permission
handler (`classify_mcp_server` in
`src/commands/messaging/notifications.rs`) requires:

- **kind absent or `"other"`** — what agents report for MCP tools. kiro-cli
  titles shell runs `Running: <command>`, so a command spelled
  `@builder-mcp/InternalSearch` produces an MCP-looking title; its kind is
  `execute`, and an `execute` (or any other built-in kind) call is never an
  MCP call.
- **titles agree** — the call's first `tool_call` title (the policy key)
  and the permission request's own title must name the same server.

The frontend never parses titles: it only offers server trust when the
`permission_request` payload carries `mcpServer`.

## Where it lives in the code

- **Config** (`src/config/permissions.rs`) — `ToolPermissionsConfig`
  carries `trust_all`, `terminator_mode`, `trusted_kinds`,
  `trusted_mcp_servers`, and `tools: Vec<ToolPolicy>`;
  `resolve_policy_for(title, kind, mcp_server)` applies the precedence
  below.
- **Per-tool policy** (`ToolPolicy`) — `policy` is `"ask" | "allow" |
  "deny"`; `grant_type` is `"once" | "24h" | "always"`; `granted_at` and
  `last_seen` are ISO 8601 timestamps. `effective_policy()` resolves the
  raw fields against the current time so that a 24h grant lapses, an
  "always" grant goes stale after 30 days, and a future-dated timestamp
  (clock skew) is treated as suspicious and demotes back to `ask`.
- **Permission handler** (`src/commands/messaging.rs`) — the ACP
  notification handler reads `effective_policy()`, returns a synthesized
  approval to ACP if it's `allow` or `deny`, and otherwise emits the
  `permission_request` Tauri event so the frontend can prompt.
- **Audit log** (`src/permission_audit.rs`) — every grant, deny, revoke,
  expiry, and terminator-mode toggle is appended to
  `<config_dir>/kage/permission-audit.jsonl`. Documented in
  `SECURITY_MODEL.md` as informational, not tamper-evident.
- **Settings UI** (`ui/js/settings/tool-permissions.js`) — manages the
  toggles plus a list view of granted tools. Revoking from the list
  writes a `Revoked` audit entry.
- **Modal** (`ui/js/floating/permissions.js` and
  `ui/js/chat/permissions.js`) — listens for `permission_request`
  events, gates the modal by current session id, sends the user's
  decision back through `send_permission_response`.

## Permission flow

1. Agent calls a tool; ACP sends `session/request_permission`.
2. The notification handler resolves the tool's title and ACP kind
   (the request's `toolCall.kind`, else the kind from that call's earlier
   `tool_call` / `tool_call_update`). Only an explicit kind counts — ACP
   treats a missing kind as "other", and argument-sniffing is unsafe
   (kiro-cli's file-write tool sends `{"command": "create"}`). It also
   identifies the tool's MCP server, if any (see above). It then reads
   `ToolPolicy::effective_policy()` for the title.
3. An explicit `deny` wins. Otherwise, if `trust_all` or
   `terminator_mode` is on, the kind is in `trusted_kinds`, or the MCP
   server is in `trusted_mcp_servers`, the handler returns `allow_once`
   immediately and writes a `Granted` audit entry (with the command in
   `args_preview` for shell runs, the tool's arguments for MCP tools).
   Trusted-kind and trusted-server calls don't add a per-tool row to
   `tools`, and `trusted_kinds` / `trusted_mcp_servers` are ignored for
   background / ephemeral prompt sessions. The prompt doesn't bulk-approve
   queued requests from those sessions either (`backgroundSession` in the
   `permission_request` payload).
4. If the policy resolves to `allow`, the handler returns `allow_once`
   immediately. The grant_type is preserved for the next call.
5. If the policy resolves to `deny`, the handler returns `reject_once`.
6. Otherwise, the handler emits `permission_request` to the frontend.
   The modal collects the user's choice and calls
   `send_permission_response`, which writes the new policy back to
   config (atomically — see P0.5 in the audit) and writes the audit
   entry.

## Example ACP `session/request_permission`

```json
{
  "jsonrpc": "2.0",
  "method": "session/request_permission",
  "params": {
    "sessionId": "054b43cd-e53e-4ea9-be6f-7e3db5a3395b",
    "toolCall": {
      "toolCallId": "tooluse_aAEsCgdNNuc0gpp1PkIhjz",
      "title": "Searching the web"
    },
    "options": [
      { "optionId": "allow_once",   "name": "Yes",     "kind": "allow_once"   },
      { "optionId": "allow_always", "name": "Always",  "kind": "allow_always" },
      { "optionId": "reject_once",  "name": "No",      "kind": "reject_once"  }
    ]
  },
  "id": "b8c39264-fe39-49a9-9c5e-dd9e4934f3df"
}
```

## Extension capabilities

Extensions are a separate trust system: each one declares the
capabilities it needs (storage, network, etc.) and the user approves
the set at install time. Capability grants live in `extension_grants`
(keyed by extension id) in config; the sandbox host enforces them on
every IPC `invoke` from the extension iframe. See `SECURITY_MODEL.md`
for the threat model.

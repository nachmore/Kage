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

Two settings change this prompt flow:

- **Trust mode (`trust_all`)** — auto-approves every tool call without
  prompting, except those with an explicit `deny` policy.
- **Terminator mode (`terminator_mode`)** — same as `trust_all` but
  intended as a temporary "leave me alone" toggle. Toggling it on/off
  is recorded in the audit log.

Both default to off. Both surface a warning in Settings when enabled.

## Where it lives in the code

- **Config** (`src/config.rs`) — `ToolPermissionsConfig` carries
  `trust_all`, `terminator_mode`, and `tools: Vec<ToolPolicy>`.
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
2. The notification handler reads `ToolPolicy::effective_policy()` for
   the tool title.
3. If `trust_all` or `terminator_mode` is on, the handler returns
   `allow_once` immediately and writes a `Granted` audit entry.
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

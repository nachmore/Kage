# Computer Control: Why It's a Separate MCP Binary

`kage-computer-control-mcp` is a standalone executable built from
`src/mcp_sidecar.rs`. It ships alongside the main `kage`
binary, is registered in `mcp.json` by `src/mcp_registration.rs`, and is
spawned over stdio by the agent backend (e.g. `kiro-cli`) as a child
process — not by Kage itself.

This doc records **why** that responsibility lives in its own process
rather than folded into the main binary.

## Two paths tools can reach the agent

Kage exposes agent-callable tools through two distinct mechanisms:

1. **MCP servers** — separate processes registered in `mcp.json` and
   spawned by the agent over stdio. JSON-RPC 2.0 on the wire. This is what
   computer-control uses.
2. **Local extensions** — JS in the WebView declares tools in its
   manifest. `send_extension_tool_steering` pushes those declarations to
   the agent as a hidden steering message. The agent emits a
   specially-formatted call in its response stream; the frontend detects
   it (`detectExtensionToolCall` in `ui/js/shared/streaming-utils.js`),
   runs the extension's JS handler, and returns the result via the
   `extension_tool_response` Tauri command, which `send_chat_streaming`s
   it back to the agent as a follow-up message.

Extensions are therefore **not frontend-only**. They can absolutely
serve tools to the agent. So "fold computer-control into the main binary
as an extension" is a real architectural option on paper.

It's still the wrong fit. The reason is threading, not packaging.

## The real reason: OS accessibility APIs demand a dedicated thread

Both of the accessibility providers Kage ships already route every call
through a dedicated worker thread:

- Windows: `src/os/windows/uia_worker.rs` (thread `acp-uia-worker`)
- macOS:   `src/os/macos/ax_worker.rs`     (thread `acp-ax-worker`)

Their header comments explain why in detail, but the short version:

- Native element handles (`UiaElement` on Windows, `AXUIElementRef` on
  macOS) are either `!Send` or have lifetime rules that don't survive
  being moved between arbitrary threads.
- The provider keeps a `thread_local!` registry mapping the ephemeral
  IDs handed back to the LLM → native handles. If two different threads
  register and resolve IDs, each gets its own `thread_local!` slot and
  lookups silently return empty.
- Windows UIA additionally requires a single-threaded apartment
  (`CoInitializeEx(COINIT_APARTMENTTHREADED)`) and is happiest with
  serial access. The worker owns the apartment and the cached
  `UIAutomation` + `UITreeWalker` objects for the life of the process.

The extension-tool pathway doesn't give us that thread. Extensions run
their handlers in the WebView's JS runtime, results flow through the
Tauri command bus and Tauri's `spawn_blocking` pool, and any native
work ends up on whichever thread the runtime happened to pick. The
`thread_local!` registry problem is real and has been hit during
development — `uia_worker.rs` calls it out explicitly:

> Pre-2026-05 the public functions were called directly from whatever
> thread happened to be invoking them — Tauri's `spawn_blocking` pool
> in the main app, the stdin loop in the MCP binary. The MCP binary is
> single-threaded so it accidentally got the right behaviour.

The dedicated MCP binary is a clean way to guarantee that threading
model: its stdin-reader loop is single-threaded, every tool call is
dispatched serially on that thread, and the worker thread behind it
owns the COM apartment and the registry for the life of the process.

Folding the tools into the extension pathway would force every caller
to go back through the worker-channel indirection on arbitrary Tauri
threads. That already works (the library supports it — the main binary
calls `folder_tools` directly the same way), but it puts heavy, slow,
occasionally-hanging UIA work on the same process as the UI event loop.

## Secondary benefits of the separation

These wouldn't justify the split on their own, but they reinforce it:

- **Crash isolation.** UIA and AX can hang on pathological windows, and
  `SendInput` sits on top of Win32 kernel paths that have their own
  failure modes. A crash in the MCP child kills just that child; the
  agent respawns it. Folded into the GUI process, the same crash would
  take down chat, tray, hotkeys, settings, everything.
- **macOS TCC permission scope.** Accessibility and Screen Recording
  grants attach per-executable. A dedicated automation binary can
  request those permissions narrowly without escalating the main app's
  permission footprint.
- **Iteration loop.** `cargo build --bin kage-computer-control-mcp`,
  kill the old child, the agent respawns the new one — no GUI restart,
  no chat state loss. The dev-only papercut is that `cargo tauri dev`
  and `cargo check` don't rebuild the MCP binary. That's a build
  ergonomics issue, not an architectural one.

## What the separation costs

Very little in terms of code: both binaries link the same `kage`
library, so `computer_control`, `os::accessibility`, `mcp_json_rpc`,
`mcp_registration`, and `commands::folder_tools` are all shared. The
MCP binary's unique surface is ~1000 lines of JSON-RPC dispatch in
`src/mcp_sidecar.rs` and nothing more.

The operational costs are:

- One extra process in Task Manager / Activity Monitor per session.
- A build step that has to rebuild a second binary when the shared
  code changes (documented in `CLAUDE.md` → "Two binaries").
- Registration logic in `mcp_registration.rs` to install/upgrade the
  entry in `mcp.json`.

All tolerable.

## When an extension is the right call instead

Extensions are a good fit for tools that:

- Render or interact with the WebView UI (preview modals, inline
  visualisations, form handlers).
- Wrap pure-JS logic with no native dependencies.
- Call user-facing HTTP APIs where latency, crash isolation, and
  apartment threading don't matter.

They're the wrong fit for tools that:

- Touch OS accessibility APIs (Windows UIA, macOS AX).
- Need a specific COM apartment or runloop affinity.
- Do long-running or hang-prone native work that you don't want
  sharing a process with the UI event loop.
- Need per-binary OS permission grants (macOS TCC).

When those constraints apply, a separate MCP binary is the right shape,
and the current computer-control split is the canonical example.

## Pointers

- Entry point: `src/mcp_sidecar.rs`
- MCP registration: `src/mcp_registration.rs`
- Windows worker: `src/os/windows/uia_worker.rs`
- macOS worker: `src/os/macos/ax_worker.rs`
- Extension-tool pathway: `src/commands/messaging/permissions.rs`
  (`extension_tool_response`, `send_extension_tool_steering`) and
  `ui/js/shared/extension-tool-controller.js`
- ACP transport: `src/acp_client/transport.rs`

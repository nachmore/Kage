# Security Model

## Content Security Policy (CSP)

The Tauri webview runs with `"csp": null` (CSP disabled). This is an intentional
design decision, not an oversight.

### Why CSP is disabled

Kiro Assistant's webview does not load external web content. All content sources are:

- **Agent responses** — from the trusted ACP backend (kiro-cli), rendered as markdown
- **Local UI** — HTML/JS/CSS bundled into the binary at compile time
- **Extensions** — installed from a controlled store or local directories
- **Agent-produced JS** — intentionally executed in a sandboxed eval context

In a typical web application, CSP prevents cross-site scripting (XSS) where
untrusted user input is injected into the page. In Kiro Assistant:

1. There are no untrusted content sources — no web browsing, no external URLs
2. The agent already has more power through its tool access (file I/O, shell
   commands, computer control) than any injected script could gain via the
   Tauri IPC bridge
3. The real security boundary is the **tool permission system** — users are
   prompted before the agent executes any tool
4. A strict CSP would break legitimate features (inline styles for theming,
   eval for agent-produced JS, vendor libraries)

### When to revisit

CSP should be reconsidered if any of these change:

- The webview loads external URLs (link previews, web content rendering)
- Untrusted third-party content is rendered in the webview
- The extension system allows arbitrary HTML/JS from unknown sources

### Defense in depth

Even without CSP, the following mitigations are in place:

- Markdown rendering sanitizes HTML document markers (`<!`, `<html>`) to prevent
  raw HTML passthrough (see steering rule in `structure.md`)
- Tool permissions require explicit user approval before execution
- The computer-control MCP server is opt-in (enabled during first-run setup)
- Extension installation requires user action

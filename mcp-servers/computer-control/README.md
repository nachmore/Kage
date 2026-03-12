# Computer Control MCP Server v2 — Accessibility-First

An MCP server that gives an AI agent structured access to desktop UI elements
through OS accessibility APIs, with mouse/keyboard/screenshot fallback.

## How it works

Instead of taking screenshots and guessing pixel coordinates, the agent reads
a structured tree of UI elements (buttons, menus, text fields, etc.) and
interacts with them directly through the OS accessibility API.

```
Agent: get_ui_tree()
→ [window] "Notepad" {e1}
    [menubar] {e2}
      [menuitem] "File" {e3} actions=[invoke]
    [edit] "Text Editor" {e4} value="" actions=[set_value]

Agent: click_element("e3")
→ Invoked [menuitem] 'File'

Agent: get_ui_tree()
→ [menu] "File" {e10}
    [menuitem] "New" {e11} actions=[invoke]
    [menuitem] "Open..." {e12} actions=[invoke]
    [menuitem] "Save" {e13} actions=[invoke]
```

## Setup

### Prerequisites
- Python 3.10+
- `uv` (recommended) or `pip`

### Install
```bash
cd mcp-servers/computer-control
uv pip install -e .
```

This installs `uiautomation` on Windows automatically. macOS and Linux
providers are stubbed out (contributions welcome).

### MCP config

Add to `~/.kiro/settings/mcp.json`:

```json
{
  "mcpServers": {
    "computer-control": {
      "command": "python",
      "args": ["-m", "computer_control.server"],
      "cwd": "<full-path-to>/mcp-servers/computer-control",
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

## Tools

### Primary (accessibility-based)

| Tool | Description |
|------|-------------|
| `get_ui_tree` | Get structured element tree for a window |
| `find_elements` | Search for elements by name, role, or automation ID |
| `get_focused_element` | Get the currently focused element |
| `list_windows` | List visible (non-minimized) top-level windows |
| `list_all_windows` | List ALL top-level windows including minimized |
| `click_element` | Click/invoke an element by ID |
| `set_value` | Set text/value on an element by ID |
| `toggle_element` | Toggle a checkbox/switch by ID |
| `select_element` | Select a list/tab item by ID |
| `expand_element` | Expand a menu/tree node by ID |
| `collapse_element` | Collapse a menu/tree node by ID |
| `scroll_element` | Scroll within a container by ID |
| `get_element_text` | Read text content from an element |

### Fallback (coordinate/pixel-based)

| Tool | Description |
|------|-------------|
| `screenshot` | Capture screen (use when tree is insufficient) |
| `click` | Click at screen coordinates |
| `type_text` | Type text via keyboard |
| `key_press` | Press key combinations |
| `key_press_confirmed` | Execute dangerous key combos |
| `launch_app` | Start an application |
| `drag` | Click and drag between points |
| `scroll` | Scroll at coordinates |
| `move_mouse` | Move cursor |
| `wait` | Pause between actions |
| `get_cursor_position` | Get cursor position |
| `get_screen_size` | Get monitor resolution |

## Platform support

| Platform | Provider | Status |
|----------|----------|--------|
| Windows | UI Automation (IUIAutomation via comtypes) | ✅ Implemented |
| macOS | Accessibility API (AXUIElement via pyobjc) | 🔲 Stub |
| Linux | AT-SPI2 (D-Bus via pyatspi2) | 🔲 Stub |

## Architecture

See [docs/COMPUTER_CONTROL_V2.md](../../docs/COMPUTER_CONTROL_V2.md) for the
full design document.

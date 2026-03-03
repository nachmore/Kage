# Computer Control MCP Server

An MCP server that gives an AI agent the ability to control your computer — move the mouse, click, type, press key combos, take screenshots, and launch apps.

## Setup

### Prerequisites
- Python 3.10+
- `uv` (recommended) or `pip`

### Install dependencies
```bash
cd mcp-servers/computer-control
uv pip install -e .
```

### Add to MCP config

Add this to your `~/.kiro/settings/mcp.json` (or workspace `.kiro/settings/mcp.json`):

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

Or if installed via pip/uv:
```json
{
  "mcpServers": {
    "computer-control": {
      "command": "computer-control-mcp",
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

## Available Tools

| Tool | Description |
|------|-------------|
| `screenshot` | Capture full screen or a region (returns image) |
| `get_screen_size` | Get primary monitor resolution |
| `move_mouse` | Move cursor to absolute position |
| `click` | Click at position (left/right/middle, single/double) |
| `double_click` | Double-click at position |
| `right_click` | Right-click at position |
| `drag` | Click and drag between two points |
| `scroll` | Scroll up/down |
| `type_text` | Type a string of text |
| `key_press` | Press key combos (e.g. `ctrl+s`) |
| `key_press_confirmed` | Execute dangerous key combos after confirmation |
| `launch_app` | Launch an application by name |
| `wait` | Pause between actions |
| `get_cursor_position` | Get current cursor position |

## Safety

Dangerous key combinations (like `alt+f4`, `ctrl+alt+delete`, `win+l`) are blocked by `key_press` and require the agent to explicitly call `key_press_confirmed` with `confirm=True`. This gives the tool permission system a chance to intervene.

## How the agent uses it

1. User asks: "open paint and draw a circle"
2. Agent calls `launch_app("mspaint")`
3. Agent calls `wait(1000)` to let Paint open
4. Agent calls `screenshot()` to see the current state
5. Agent identifies the canvas area from the screenshot
6. Agent uses `click`, `drag`, etc. to draw
7. Agent calls `screenshot()` again to verify the result

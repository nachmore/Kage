# Computer Control v2: Accessibility-First Architecture

## Research Summary

### The Problem with Screenshot-Based Control

The current `computer-control` MCP server relies on a screenshot loop:
1. Take screenshot → send image to LLM → LLM guesses coordinates → execute action → repeat
2. Each screenshot is ~50-100KB JPEG, consuming ~1000+ tokens per image
3. Coordinate guessing is inherently imprecise (wrong button, off-by-pixels)
4. The full conversation history (with all those images) gets passed back every turn
5. Latency: capture → encode → transmit → decode → reason → respond ≈ 3-8 seconds per step
6. No semantic understanding — the agent sees pixels, not "Save button" or "File menu"

### The Alternative: OS Accessibility APIs

Every major OS exposes a structured tree of UI elements through accessibility APIs designed
for screen readers. These APIs provide: element names, types, states, positions, and
available actions — all as lightweight text.

#### Windows: UI Automation (UIA)
- COM-based API (`IUIAutomation`) available since Windows Vista
- Exposes every UI element as a node: Name, ControlType, AutomationId, BoundingRectangle,
  IsEnabled, patterns (Invoke, Value, Toggle, Selection, etc.)
- Supports: Win32, WinForms, WPF, UWP/WinUI, Qt, Electron (with
  `--force-renderer-accessibility`), browsers
- Python access via `uiautomation` package (wraps COM via comtypes)
- Tree walk: ~50-200ms to enumerate a window's controls
- Direct interaction: `InvokePattern.Invoke()`, `ValuePattern.SetValue()`,
  `SelectionItemPattern.Select()`

#### macOS: Accessibility API (AX)
- C-based API using `AXUIElementRef` from ApplicationServices framework
- Properties: AXRole, AXTitle, AXDescription, AXValue, AXPosition, AXSize, AXChildren
- Actions: AXPress, AXConfirm, AXCancel, AXRaise, AXShowMenu
- Python access via `pyobjc` (ApplicationServices bridge)
- Requires Accessibility permission (System Settings → Privacy → Accessibility)

#### Linux: AT-SPI2
- D-Bus protocol (`org.a11y.atspi`) used by screen readers like Orca
- Properties: Role, Name, Description, State; interfaces: Action, Text, Value, Selection
- Python access via `pyatspi2` (official bindings)
- Requires accessibility bus running (usually started by desktop session)

### Structured Tree vs Screenshots: The Numbers

| Metric | Screenshot approach | Accessibility tree |
|--------|-------------------|--------------------|
| Data per step | ~50-100KB image (~1000+ tokens) | ~1-5KB text (~200-500 tokens) |
| Precision | Pixel guessing (error-prone) | Exact element targeting |
| Speed per step | 3-8s (capture+encode+reason) | 0.5-2s (query+reason) |
| Semantic info | None (just pixels) | Name, type, state, actions |
| Interaction | Mouse coordinates | Direct API calls |
| Reliability | Low (resolution/scaling issues) | High (API-level targeting) |

---

## Architecture

### Responsibility Split

The MCP server is a tool provider — it has no LLM, no reasoning capability. It exposes
primitives. The agent (the LLM caller) owns all planning and orchestration.

For multi-step UI tasks, the main agent decomposes the goal into discrete steps and
delegates each step to a sub-agent via the ACP server's native `invoke_subagents`
capability. Each sub-agent runs with a fresh context containing only its specific task.

```
┌─────────────────────────────────────────────────────────┐
│  Main Agent (LLM)                                       │
│                                                         │
│  - Receives user goal                                   │
│  - Decomposes into discrete steps                       │
│  - Delegates each step to a sub-agent                   │
│  - Collects results, decides next step                  │
│  - Reports completion to user                           │
│                                                         │
│  Main session stays clean — just task list + results.   │
└──────────────────┬──────────────────────────────────────┘
                   │ invoke_subagents
                   ▼
┌─────────────────────────────────────────────────────────┐
│  Sub-Agent (fresh context per step)                     │
│                                                         │
│  - Receives: specific task description                  │
│  - Calls MCP tools to perceive UI (get_ui_tree)         │
│  - Reasons about what action to take                    │
│  - Calls MCP tools to execute (click_element, etc.)     │
│  - Verifies result via get_ui_tree                      │
│  - Reports success/failure back to main agent           │
│                                                         │
│  Context is discarded after the step completes.         │
└──────────────────┬──────────────────────────────────────┘
                   │ MCP tool calls
                   ▼
┌─────────────────────────────────────────────────────────┐
│  MCP Server (computer-control v2)                       │
│                                                         │
│  Perception:                                            │
│    - get_ui_tree()     → structured element tree        │
│    - find_elements()   → search by criteria             │
│    - get_focused()     → current focus + context        │
│    - list_windows()    → visible (non-minimized) windows   │
│    - list_all_windows()→ ALL windows incl. minimized      │
│                                                         │
│  Action:                                                │
│    - click_element()   → invoke/press via API           │
│    - set_value()       → set text/value via API         │
│    - select_element()  → select item via API            │
│    - toggle_element()  → toggle checkbox/switch         │
│    - expand/collapse() → tree nodes, menus              │
│    - scroll_element()  → scroll containers              │
│                                                         │
│  Fallback (for apps with poor a11y support):            │
│    - click(x, y)       → coordinate click               │
│    - type_text(text)   → keyboard input                 │
│    - key_press(keys)   → key combos                     │
│    - screenshot()      → visual capture                 │
│    - launch_app(name)  → start application              │
│                                                         │
│  No LLM. No planning. Just tools.                       │
└─────────────────────────────────────────────────────────┘
```

### Sub-Agent Orchestration

The ACP server natively supports sub-agent invocation. The main agent delegates
UI automation steps to sub-agents, each running with a fresh context window.

Benefits:
- Main session context stays small (just task descriptions + results)
- Each step gets only the UI state it needs — no accumulated screenshots/trees
- Failed steps can be retried without polluting the main context
- Steps are naturally isolated — one step's errors don't confuse the next

Example flow:
```
User: "Open Calculator and compute 42 × 17"

Main agent decomposes:
  Step 1 → sub-agent: "Launch Calculator, verify it opened"
  Step 2 → sub-agent: "In Calculator, press 4,2,×,1,7,= and read the display"

Each sub-agent:
  - Gets fresh context (no history from other steps)
  - Calls get_ui_tree(), find_elements(), click_element() as needed
  - Reports result back to main agent
  - Context is discarded
```

For simple single-step operations (e.g. "list windows", "click Save button"),
the main agent calls tools directly without sub-agents.

---

## MCP Tool Specifications

### Perception Tools

#### `get_ui_tree`
Returns the accessibility tree for a window.

```
Parameters:
  window_title: str (optional) — substring match; uses focused window if omitted
  max_depth: int = 3 — how deep to walk the tree
  include_invisible: bool = false — include offscreen/hidden elements

Returns: text tree (compact, token-efficient format)

Example output:
  [window] "Untitled - Notepad" {id:w1} (1200x800)
    [menubar] {id:m1}
      [menuitem] "File" {id:m2} actions=[invoke]
      [menuitem] "Edit" {id:m3} actions=[invoke]
      [menuitem] "View" {id:m4} actions=[invoke]
    [edit] "Text Editor" {id:e1} value="" actions=[set_value,focus]
    [statusbar] {id:s1}
      [text] "Ln 1, Col 1" {id:s2}
```

Element IDs are ephemeral (valid only for the current tree snapshot). The agent
must re-query the tree after actions that change the UI.

#### `find_elements`
Search for elements matching criteria within a window.

```
Parameters:
  name: str (optional) — substring match on element name
  role: str (optional) — element type (button, edit, menuitem, etc.)
  automation_id: str (optional) — developer-assigned ID (Windows UIA)
  value: str (optional) — current value substring
  window_title: str (optional) — target window

Returns: flat list of matching elements with id, role, name, value, state, bounds
```

#### `get_focused_element`
Returns the currently focused element and its parent chain.

```
Parameters: none

Returns: focused element details + ancestor path to window root
```

#### `list_windows`
Returns visible (non-minimized) top-level windows. Minimized windows are excluded.
For a complete list including minimized windows, use `list_all_windows`.

```
Parameters:
  title_filter: str (optional) — substring match

Returns: list of {title, bounds, process_name, has_accessibility_tree}
```

### Action Tools

All action tools take an `element_id` from a previous `get_ui_tree` or `find_elements` call.

#### `click_element`
Invoke/press an element via accessibility API (not mouse coordinates).

```
Parameters:
  element_id: str — from tree query
  
Returns: "Clicked [role] 'name'" or error
```

#### `set_value`
Set text or value on an element (text fields, spinners, etc.).

```
Parameters:
  element_id: str
  value: str

Returns: "Set value on [role] 'name'" or error
```

#### `toggle_element`
Toggle a checkbox, switch, or toggle button.

```
Parameters:
  element_id: str

Returns: "Toggled [role] 'name' → {on|off}" or error
```

#### `select_element`
Select an item in a list, combo box, tab control, etc.

```
Parameters:
  element_id: str

Returns: "Selected [role] 'name'" or error
```

#### `expand_element` / `collapse_element`
Expand or collapse tree nodes, menus, dropdowns, combo boxes.

```
Parameters:
  element_id: str

Returns: "Expanded/Collapsed [role] 'name'" or error
```

#### `scroll_element`
Scroll within a scrollable container.

```
Parameters:
  element_id: str
  direction: "up" | "down" | "left" | "right"
  amount: float = 0.2 — fraction of viewport (0.0-1.0)

Returns: "Scrolled [direction] on [role] 'name'" or error
```

#### `get_element_text`
Read text content from a text element (for reading documents, code editors, etc.).

```
Parameters:
  element_id: str

Returns: text content of the element
```

### Fallback Tools (for apps with poor accessibility)

These are the existing low-level tools, kept for when the accessibility tree
is empty or insufficient:

- `click(x, y, button, count)` — coordinate-based click
- `type_text(text, interval)` — keyboard typing
- `key_press(keys)` — key combinations
- `key_press_confirmed(keys, confirm)` — dangerous key combos
- `screenshot(region, max_width)` — screen capture
- `launch_app(name)` — start an application
- `drag(from_x, from_y, to_x, to_y)` — click and drag
- `scroll(direction, amount, x, y)` — coordinate-based scroll
- `move_mouse(x, y)` — move cursor
- `wait(milliseconds)` — pause
- `get_cursor_position()` — cursor location
- `get_screen_size()` — monitor resolution

---

## File Structure

```
mcp-servers/computer-control/
  computer_control/
    server.py                # MCP server — registers all tools
    accessibility/
      __init__.py
      base.py                # Abstract AccessibilityProvider class
      windows_uia.py         # Windows UI Automation (IUIAutomation via comtypes)
      macos_ax.py            # macOS AXUIElement via pyobjc
      linux_atspi.py         # Linux AT-SPI2 via pyatspi2
      tree.py                # UIElement dataclass, tree serialization, ID management
    actions/
      __init__.py
      element.py             # Element-based actions (click, set_value, toggle, etc.)
      fallback.py            # Coordinate-based mouse/keyboard (existing functionality)
    platform.py              # Platform detection, provider factory
  pyproject.toml
  README.md
```

---

## Dependencies

**Windows:**
- `uiautomation>=2.0.18` — MS UI Automation COM wrapper
- `comtypes` — COM access (pulled in by uiautomation)

**macOS:**
- `pyobjc-framework-ApplicationServices` — AXUIElement
- `pyobjc-framework-Cocoa` — NSWorkspace, NSRunningApplication

**Linux:**
- `pyatspi` — AT-SPI2 bindings (system package, not pip)
- `dbus-python` — D-Bus fallback

**Cross-platform:**
- `mcp[cli]` — MCP server framework
- `mss` — Screen capture (fallback)
- `Pillow` — Image processing (fallback)
- `pyautogui` — Mouse/keyboard (fallback)

---

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Some apps have no accessibility tree | Fallback tools remain available; agent detects empty tree |
| Electron apps need `--force-renderer-accessibility` | Detect Electron process and warn in tool output |
| macOS requires Accessibility permission | Clear error message with instructions on first use |
| Linux AT-SPI2 may not be running | Auto-detect; document required env vars |
| Tree too deep/large for complex apps | `max_depth` parameter; prune invisible/offscreen |
| Element IDs not stable across queries | IDs are ephemeral by design; re-query after actions |
| Cross-platform API differences | Abstract base class with per-platform implementations |

---

## Implementation Order

1. **Core data structures** — `tree.py`: UIElement dataclass, tree serialization, ID registry
2. **Windows provider** — `windows_uia.py`: tree walking + element interaction via UIA
3. **Perception tools** — `get_ui_tree`, `find_elements`, `get_focused_element`, `list_windows`, `list_all_windows`
4. **Action tools** — `click_element`, `set_value`, `toggle_element`, etc.
5. **Fallback tools** — migrate existing mouse/keyboard/screenshot into `fallback.py`
6. **Server integration** — wire everything into `server.py`
7. **macOS provider** — `macos_ax.py`
8. **Linux provider** — `linux_atspi.py`
9. **README + docs**

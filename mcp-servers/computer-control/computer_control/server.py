"""
Computer Control MCP Server v2 — Accessibility-First Architecture

Provides tools for an AI agent to control the computer through
OS accessibility APIs (UI Automation on Windows, AX on macOS, AT-SPI2 on Linux).

Primary tools use structured accessibility trees for perception and direct
element interaction for actions. Fallback tools (mouse/keyboard/screenshot)
are available for apps with poor accessibility support.
"""

import base64
import io
import logging
import os
import platform
import subprocess
import time
from typing import Optional

# ---------------------------------------------------------------------------
# Logging setup (file only — never write to stdout/stderr, it's JSON-RPC)
# ---------------------------------------------------------------------------
LOG_DIR = os.path.join(os.path.expanduser("~"), ".kiro", "logs")
os.makedirs(LOG_DIR, exist_ok=True)
LOG_FILE = os.path.join(LOG_DIR, "computer-control-mcp.log")

_file_handler = logging.FileHandler(LOG_FILE, mode="a", encoding="utf-8")
_file_handler.setLevel(logging.DEBUG)
_file_handler.setFormatter(
    logging.Formatter("%(asctime)s [%(levelname)s] %(name)s — %(message)s")
)

log = logging.getLogger("computer-control")
log.setLevel(logging.DEBUG)
log.addHandler(_file_handler)
log.propagate = False

log.info("=" * 60)
log.info("Computer Control MCP v2 starting (pid=%d)", os.getpid())

# ---------------------------------------------------------------------------
# Imports
# ---------------------------------------------------------------------------
import mss
import mss.tools
from mcp.server.fastmcp import FastMCP
from mcp.types import ImageContent, TextContent
from PIL import Image as PILImage
from pynput.keyboard import Controller as KeyboardController
from pynput.keyboard import Key
from pynput.mouse import Button
from pynput.mouse import Controller as MouseController

from .accessibility import get_provider, ElementID
from .accessibility.base import AccessibilityProvider

# ---------------------------------------------------------------------------
# Initialize accessibility provider
# ---------------------------------------------------------------------------
_provider: Optional[AccessibilityProvider] = None
try:
    _provider = get_provider()
    log.info("Accessibility provider loaded: %s", type(_provider).__name__)
except Exception as e:
    log.warning("No accessibility provider available: %s", e)
    log.warning("Only fallback tools (mouse/keyboard/screenshot) will work.")

# ---------------------------------------------------------------------------
# MCP Server
# ---------------------------------------------------------------------------
mcp = FastMCP(
    "computer-control",
    instructions="""\
Control the computer through OS accessibility APIs. This gives you structured \
access to UI elements (buttons, menus, text fields, etc.) instead of raw pixels.

CRITICAL — PLANNING vs EXECUTION:
For complex UI automation tasks (2+ steps), you MUST follow this approach:

Output a structured task list as a JSON code block. The client will automatically \
detect and execute it step by step using sub-agents. Do NOT execute the steps yourself.

Format your response like this:
```automation_plan
[
  {"step": 1, "task": "Launch Calculator", "details": "Use launch_and_get_tree('calc')"},
  {"step": 2, "task": "Press 9 × 3 =", "details": "Click Nine, Multiply by, Three, Equals"},
  {"step": 3, "task": "Read result", "details": "Use find_elements(name='Display') to read the value"}
]
```

The client will execute each step automatically using sub-agents with fresh context. \
You do NOT need to call any tools or invoke sub-agents yourself for multi-step tasks.

For SIMPLE single-step tasks (e.g. "what windows are open?", "click Save"), \
skip planning and just call the tool directly.

PERFORMANCE — use compound tools to minimize round-trips:
Each LLM round-trip costs time and tokens. Mention these in step details:

- launch_and_get_tree(app_name) — launches app + waits + returns UI tree (saves 2 trips)
- click_and_get_tree(element_id) — clicks + returns updated tree (saves 1 trip)
- click_and_read_result(element_id, result_name) — clicks + reads result (saves 2 trips)
- type_and_get_tree(element_id, text) — types + returns updated tree (saves 1 trip)

NEVER use screenshot() for verification. Use get_ui_tree() or find_elements() instead.

Element IDs (like {e42}) are ephemeral — only valid until the next tree-returning call.

FALLBACK — for apps with poor accessibility support:
If get_ui_tree() returns an empty tree, fall back to screenshot-based approach.
""",
)

mouse = MouseController()
keyboard = KeyboardController()


def _require_provider() -> AccessibilityProvider:
    """Get the accessibility provider or raise a clear error."""
    if _provider is None:
        raise RuntimeError(
            "No accessibility provider available on this platform. "
            "Use fallback tools (screenshot, click, type_text) instead."
        )
    return _provider


# ===================================================================
# ACCESSIBILITY TOOLS — Primary
# ===================================================================


@mcp.tool()
def get_ui_tree(
    window_title: Optional[str] = None,
    max_depth: int = 3,
    include_invisible: bool = False,
) -> str:
    """Get the accessibility tree for a window — your primary way to 'see' the UI.

    Returns a structured text tree of UI elements with their roles, names,
    values, states, and available actions. Each element has an {id} you can
    use with action tools like click_element() or set_value().

    Element IDs are ephemeral — they reset on each call. Always get fresh
    IDs before performing actions.

    Args:
        window_title: Substring match on window title. Uses focused window if omitted.
        max_depth: How deep to walk the tree (default 3). Increase for complex UIs.
        include_invisible: Include offscreen/hidden elements (default false).

    Returns:
        Text tree of UI elements. Example:
        [window] "Notepad" {e1} (1200x800@100,50)
          [menubar] {e2}
            [menuitem] "File" {e3} actions=[invoke]
          [edit] "Text Editor" {e4} value="" actions=[set_value,focus]
    """
    provider = _require_provider()
    log.info("get_ui_tree: title=%r, depth=%d", window_title, max_depth)
    try:
        tree = provider.get_ui_tree(window_title, max_depth, include_invisible)
        text = tree.to_text(max_depth=max_depth)

        # Surface metadata (truncation warnings, Electron hints) if present
        meta = getattr(tree, "_meta", "")
        if meta:
            text = meta + "\n\n" + text

        log.info("get_ui_tree: returned %d chars", len(text))
        return text
    except Exception as e:
        log.exception("get_ui_tree failed")
        return f"Error getting UI tree: {e}"


@mcp.tool()
def find_elements(
    name: Optional[str] = None,
    role: Optional[str] = None,
    automation_id: Optional[str] = None,
    value: Optional[str] = None,
    window_title: Optional[str] = None,
) -> str:
    """Search for UI elements matching criteria within a window.

    Returns a flat list of matching elements. Useful when you know what
    you're looking for but don't want to walk the whole tree.

    Args:
        name: Substring match on element name (e.g. "Save", "File")
        role: Element type (button, edit, menuitem, checkbox, listitem, etc.)
        automation_id: Developer-assigned ID (exact match, Windows only)
        value: Substring match on current value
        window_title: Target window (uses focused window if omitted)

    Returns:
        List of matching elements with id, role, name, value, actions.
    """
    provider = _require_provider()
    log.info("find_elements: name=%r, role=%r, aid=%r", name, role, automation_id)
    try:
        elements = provider.find_elements(name, role, automation_id, value, window_title)
        if not elements:
            return "No elements found matching the criteria."
        lines = [f"Found {len(elements)} element(s):"]
        for elem in elements:
            lines.append(elem.to_text(max_depth=0))
        # Note: if the search was truncated, the provider logs a warning
        # but we still return what we found
        if len(elements) >= 50:
            lines.append(
                f"\n⚠️ Many results ({len(elements)}). "
                f"Try narrowing your search with more specific criteria."
            )
        return "\n".join(lines)
    except Exception as e:
        log.exception("find_elements failed")
        return f"Error searching elements: {e}"


@mcp.tool()
def get_focused_element() -> str:
    """Get the currently focused UI element.

    Returns details about which element has keyboard focus, including
    its role, name, value, and available actions.
    """
    provider = _require_provider()
    log.info("get_focused_element")
    try:
        elem = provider.get_focused_element()
        if elem is None:
            return "No focused element found."
        return elem.to_text(max_depth=0)
    except Exception as e:
        log.exception("get_focused_element failed")
        return f"Error getting focused element: {e}"


@mcp.tool()
def list_windows(title_filter: Optional[str] = None) -> list[dict]:
    """List visible (non-minimized) top-level windows using the accessibility API.

    Only returns windows that are currently visible on screen — minimized windows
    are excluded. For a complete list including minimized windows, use
    list_all_windows() instead.

    Args:
        title_filter: Substring match on window title (optional).

    Returns:
        List of windows with title, bounds, and process ID.
    """
    provider = _require_provider()
    log.info("list_windows: filter=%r", title_filter)
    try:
        windows = provider.list_windows(title_filter)
        log.info("list_windows: found %d", len(windows))
        return windows
    except Exception as e:
        log.exception("list_windows failed")
        return []


@mcp.tool()
def list_all_windows(title_filter: Optional[str] = None) -> list[dict]:
    """List ALL top-level windows using the native OS window enumeration API.

    Unlike list_windows() which only shows visible (non-minimized) windows,
    this uses the OS window manager directly (EnumWindows on Windows, System
    Events on macOS, wmctrl/xdotool on Linux) and returns all top-level
    windows including minimized ones.

    Use this when you need a complete and accurate count of open windows.

    Args:
        title_filter: Substring match on window title (optional).

    Returns:
        List of windows with title, process_name, and handle.
    """
    log.info("list_all_windows: filter=%r", title_filter)
    try:
        windows = _enumerate_all_windows()
        if title_filter:
            windows = [
                w for w in windows
                if title_filter.lower() in w.get("title", "").lower()
            ]
        log.info("list_all_windows: found %d", len(windows))
        return windows
    except Exception as e:
        log.exception("list_all_windows failed")
        return []


def _enumerate_all_windows() -> list[dict]:
    """Enumerate all visible top-level windows using native OS APIs."""
    system = platform.system()
    if system == "Windows":
        return _enumerate_windows_win32()
    elif system == "Darwin":
        return _enumerate_windows_macos()
    else:
        return _enumerate_windows_linux()


def _enumerate_windows_win32() -> list[dict]:
    """Windows: Use EnumWindows via ctypes for complete window enumeration."""
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    kernel32 = ctypes.windll.kernel32

    EnumWindows = user32.EnumWindows
    IsWindowVisible = user32.IsWindowVisible
    GetWindowTextW = user32.GetWindowTextW
    GetWindowTextLengthW = user32.GetWindowTextLengthW
    GetWindowThreadProcessId = user32.GetWindowThreadProcessId
    GetWindowLongW = user32.GetWindowLongW
    GetWindow = user32.GetWindow

    GWL_EXSTYLE = -20
    WS_EX_TOOLWINDOW = 0x00000080
    GW_OWNER = 4
    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000

    results = []

    def _get_process_name(pid):
        handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
        if not handle:
            return ""
        try:
            buf = (ctypes.c_wchar * 260)()
            size = wintypes.DWORD(260)
            if kernel32.QueryFullProcessImageNameW(handle, 0, buf, ctypes.byref(size)):
                path = buf.value
                return os.path.splitext(os.path.basename(path))[0]
        finally:
            kernel32.CloseHandle(handle)
        return ""

    @ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)
    def enum_callback(hwnd, lparam):
        if not IsWindowVisible(hwnd):
            return True
        # Skip tool windows and owned windows
        ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE)
        if ex_style & WS_EX_TOOLWINDOW:
            return True
        if GetWindow(hwnd, GW_OWNER):
            return True

        length = GetWindowTextLengthW(hwnd)
        if length <= 0:
            return True
        buf = ctypes.create_unicode_buffer(length + 1)
        GetWindowTextW(hwnd, buf, length + 1)
        title = buf.value.strip()
        if not title:
            return True

        pid = wintypes.DWORD()
        GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        process_name = _get_process_name(pid.value) if pid.value else ""

        results.append({
            "title": title,
            "process_name": process_name,
            "handle": int(hwnd),
        })
        return True

    EnumWindows(enum_callback, 0)
    return results


def _enumerate_windows_macos() -> list[dict]:
    """macOS: Use osascript with System Events."""
    script = r'''
        tell application "System Events"
            set windowList to ""
            repeat with proc in (every process whose visible is true)
                set procName to name of proc
                set procId to unix id of proc
                repeat with win in (every window of proc)
                    set winTitle to name of win
                    if winTitle is not "" then
                        set windowList to windowList & procName & "\t" & winTitle & "\t" & procId & linefeed
                    end if
                end repeat
            end repeat
        end tell
        return windowList
    '''
    try:
        result = subprocess.run(
            ["osascript", "-e", script],
            capture_output=True, text=True, timeout=10,
        )
        if result.returncode != 0:
            return []
        windows = []
        for line in result.stdout.strip().split("\n"):
            parts = line.split("\t")
            if len(parts) >= 3:
                windows.append({
                    "title": parts[1],
                    "process_name": parts[0],
                    "handle": int(parts[2]) if parts[2].isdigit() else 0,
                })
        return windows
    except Exception:
        return []


def _enumerate_windows_linux() -> list[dict]:
    """Linux: Use wmctrl or xdotool."""
    # Try wmctrl first
    try:
        result = subprocess.run(
            ["wmctrl", "-l", "-p"],
            capture_output=True, text=True, timeout=5,
        )
        if result.returncode == 0:
            windows = []
            for line in result.stdout.strip().split("\n"):
                parts = line.split(None, 4)
                if len(parts) >= 5:
                    handle = int(parts[0], 16) if parts[0].startswith("0x") else 0
                    pid = int(parts[2]) if parts[2].isdigit() else 0
                    title = parts[4]
                    process_name = ""
                    if pid > 0:
                        try:
                            process_name = open(f"/proc/{pid}/comm").read().strip()
                        except Exception:
                            pass
                    if title and title != "Desktop":
                        windows.append({
                            "title": title,
                            "process_name": process_name,
                            "handle": handle,
                        })
            return windows
    except Exception:
        pass
    return []


# ===================================================================
# ACTION TOOLS — Element-based interaction
# ===================================================================


@mcp.tool()
def click_element(element_id: str) -> str:
    """Click/invoke a UI element by its ID.

    Uses the accessibility API to invoke the element directly — no mouse
    coordinates needed. Works for buttons, menu items, links, etc.

    Args:
        element_id: Element ID from get_ui_tree() or find_elements() (e.g. "e42")
    """
    provider = _require_provider()
    log.info("click_element: %s", element_id)
    try:
        handle = ElementID.resolve(element_id)
        return provider.click_element(handle)
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("click_element failed")
        return f"Error clicking element: {e}"


@mcp.tool()
def set_value(element_id: str, value: str) -> str:
    """Set the value of a text field, spinner, or other value-holding element.

    Args:
        element_id: Element ID from get_ui_tree() or find_elements()
        value: The text/value to set
    """
    provider = _require_provider()
    log.info("set_value: %s = %r", element_id, value[:100])
    try:
        handle = ElementID.resolve(element_id)
        return provider.set_value(handle, value)
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("set_value failed")
        return f"Error setting value: {e}"


@mcp.tool()
def toggle_element(element_id: str) -> str:
    """Toggle a checkbox, switch, or toggle button.

    Args:
        element_id: Element ID from get_ui_tree() or find_elements()
    """
    provider = _require_provider()
    log.info("toggle_element: %s", element_id)
    try:
        handle = ElementID.resolve(element_id)
        return provider.toggle_element(handle)
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("toggle_element failed")
        return f"Error toggling element: {e}"


@mcp.tool()
def select_element(element_id: str) -> str:
    """Select an item in a list, combo box, tab control, etc.

    Args:
        element_id: Element ID from get_ui_tree() or find_elements()
    """
    provider = _require_provider()
    log.info("select_element: %s", element_id)
    try:
        handle = ElementID.resolve(element_id)
        return provider.select_element(handle)
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("select_element failed")
        return f"Error selecting element: {e}"


@mcp.tool()
def expand_element(element_id: str) -> str:
    """Expand a tree node, menu, dropdown, or combo box.

    Args:
        element_id: Element ID from get_ui_tree() or find_elements()
    """
    provider = _require_provider()
    log.info("expand_element: %s", element_id)
    try:
        handle = ElementID.resolve(element_id)
        return provider.expand_element(handle)
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("expand_element failed")
        return f"Error expanding element: {e}"


@mcp.tool()
def collapse_element(element_id: str) -> str:
    """Collapse a tree node, menu, dropdown, or combo box.

    Args:
        element_id: Element ID from get_ui_tree() or find_elements()
    """
    provider = _require_provider()
    log.info("collapse_element: %s", element_id)
    try:
        handle = ElementID.resolve(element_id)
        return provider.collapse_element(handle)
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("collapse_element failed")
        return f"Error collapsing element: {e}"


@mcp.tool()
def scroll_element(
    element_id: str,
    direction: str = "down",
    amount: float = 0.2,
) -> str:
    """Scroll within a scrollable container.

    Args:
        element_id: Element ID of the scrollable container
        direction: "up", "down", "left", or "right"
        amount: Fraction of viewport to scroll (0.0-1.0, default 0.2)
    """
    provider = _require_provider()
    log.info("scroll_element: %s %s %.1f", element_id, direction, amount)
    try:
        handle = ElementID.resolve(element_id)
        return provider.scroll_element(handle, direction, amount)
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("scroll_element failed")
        return f"Error scrolling element: {e}"


@mcp.tool()
def get_element_text(element_id: str) -> str:
    """Read text content from a text element (documents, code editors, etc.).

    Args:
        element_id: Element ID from get_ui_tree() or find_elements()
    """
    provider = _require_provider()
    log.info("get_element_text: %s", element_id)
    try:
        handle = ElementID.resolve(element_id)
        return provider.get_element_text(handle)
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("get_element_text failed")
        return f"Error reading element text: {e}"


@mcp.tool()
def get_element_children(
    element_id: str,
    max_depth: int = 2,
) -> str:
    """Get the subtree of a specific element — drill into a part of the UI.

    Instead of re-fetching the entire window tree, use this to expand a
    specific element (e.g. a menu after expanding it, or a panel you want
    to explore deeper). Returns the element and its children.

    Note: this does NOT reset element IDs — previous IDs remain valid.
    New IDs are added for newly discovered children.

    Args:
        element_id: Element ID from a previous get_ui_tree() or find_elements()
        max_depth: How deep to walk from this element (default 2)
    """
    provider = _require_provider()
    log.info("get_element_children: %s depth=%d", element_id, max_depth)
    try:
        handle = ElementID.resolve(element_id)
        subtree = provider.get_element_children(handle, max_depth)
        text = subtree.to_text(max_depth=max_depth)
        log.info("get_element_children: returned %d chars", len(text))
        return text
    except KeyError as e:
        return str(e)
    except Exception as e:
        log.exception("get_element_children failed")
        return f"Error getting element children: {e}"


# ===================================================================
# COMPOUND TOOLS — batch multiple operations to reduce LLM round-trips
# ===================================================================


@mcp.tool()
def get_app_steering(task: str, details: str = "") -> str:
    """Get app-specific instructions for a UI automation task.

    Returns context-specific guidance for the application being automated.
    Call this at the start of a sub-agent task to get tips about the target app.

    Args:
        task: The task description (e.g. "Launch Microsoft Word")
        details: Additional task details
    """
    from .app_steering import get_steering_for_task
    log.info("get_app_steering: task=%r", task[:80])
    steering = get_steering_for_task(task, details)
    if steering:
        return steering
    return "No app-specific steering available for this task."


@mcp.tool()
def launch_and_get_tree(
    app_name: str,
    max_depth: int = 3,
    wait_seconds: float = 2.0,
) -> str:
    """Launch an app, wait for it to open, and return its UI tree — all in one call.

    This replaces the 3-step pattern: launch_app() + wait() + get_ui_tree().
    Saves 2 LLM round-trips.

    Args:
        app_name: Application name or path (e.g. 'notepad', 'calc', 'mspaint')
        max_depth: How deep to walk the UI tree (default 3)
        wait_seconds: How long to wait for the app to open (default 2.0)
    """
    system = platform.system()
    log.info("launch_and_get_tree: %r on %s", app_name, system)

    # Launch the app
    try:
        if system == "Windows":
            # Handle app names with arguments (e.g. "winword /w")
            # Don't quote the app_name so arguments are passed correctly
            proc = subprocess.Popen(
                f'start "" {app_name}', shell=True,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            try:
                _, stderr = proc.communicate(timeout=5)
                if proc.returncode != 0:
                    err = stderr.decode("utf-8", errors="replace").strip()
                    return f"Failed to launch '{app_name}': {err or 'exit code ' + str(proc.returncode)}"
            except subprocess.TimeoutExpired:
                pass
        elif system == "Darwin":
            proc = subprocess.Popen(["open", "-a", app_name], stderr=subprocess.PIPE)
            try:
                _, stderr = proc.communicate(timeout=5)
                if proc.returncode != 0:
                    return f"Failed to launch '{app_name}': {stderr.decode('utf-8', errors='replace').strip()}"
            except subprocess.TimeoutExpired:
                pass
        else:
            subprocess.Popen([app_name])
    except Exception as e:
        return f"Failed to launch '{app_name}': {e}"

    # Wait for the app to open
    time.sleep(wait_seconds)

    # Get the UI tree
    provider = _require_provider()

    # Extract the base app name for window search (strip arguments)
    search_name = app_name.split()[0].split("/")[0].split("\\")[-1]
    # Map common executable names to window title patterns
    _WINDOW_NAMES = {
        "winword": "Word", "excel": "Excel", "powerpnt": "PowerPoint",
        "outlook": "Outlook", "onenote": "OneNote",
        "notepad": "Notepad", "calc": "Calculator", "mspaint": "Paint",
        "cmd": "Command Prompt", "powershell": "PowerShell",
    }
    window_search = _WINDOW_NAMES.get(search_name.lower(), search_name)

    try:
        tree = provider.get_ui_tree(window_search, max_depth, False)
        text = tree.to_text(max_depth=max_depth)
        meta = getattr(tree, "_meta", "")
        result = f"Launched '{app_name}' successfully.\n\n"
        if meta:
            result += meta + "\n\n"
        result += text
        log.info("launch_and_get_tree: launched and got %d chars of tree", len(text))
        return result
    except Exception:
        # If we can't find by name, get the focused window
        try:
            tree = provider.get_ui_tree(None, max_depth, False)
            text = tree.to_text(max_depth=max_depth)
            return f"Launched '{app_name}'. Showing focused window tree:\n\n{text}"
        except Exception as e:
            return f"Launched '{app_name}' but could not get UI tree: {e}"


@mcp.tool()
def click_and_get_tree(
    element_id: str,
    window_title: Optional[str] = None,
    max_depth: int = 3,
) -> str:
    """Click an element and immediately return the updated UI tree — all in one call.

    This replaces the 2-step pattern: click_element() + get_ui_tree().
    Saves 1 LLM round-trip per click.

    Args:
        element_id: Element ID to click (from get_ui_tree or find_elements)
        window_title: Window to get the updated tree from (uses focused if omitted)
        max_depth: How deep to walk the updated tree (default 3)
    """
    provider = _require_provider()
    log.info("click_and_get_tree: click %s, then tree", element_id)

    # Click the element
    try:
        handle = ElementID.resolve(element_id)
        click_result = provider.click_element(handle)
    except KeyError as e:
        return str(e)
    except Exception as e:
        return f"Error clicking element: {e}"

    # Brief pause for UI to update
    time.sleep(0.3)

    # Get the updated tree
    try:
        tree = provider.get_ui_tree(window_title, max_depth, False)
        text = tree.to_text(max_depth=max_depth)
        meta = getattr(tree, "_meta", "")
        result = f"{click_result}\n\nUpdated UI tree:\n"
        if meta:
            result += meta + "\n"
        result += text
        return result
    except Exception as e:
        return f"{click_result}\n\nCould not get updated tree: {e}"


@mcp.tool()
def click_and_read_result(
    element_id: str,
    result_name: Optional[str] = None,
    result_role: Optional[str] = None,
    window_title: Optional[str] = None,
) -> str:
    """Click an element, then find and read a result element — all in one call.

    This replaces the 3-step pattern: click_element() + find_elements() + get_element_text().
    Saves 2 LLM round-trips. Useful for clicking a button and reading the output.

    Args:
        element_id: Element ID to click
        result_name: Name substring of the result element to read (e.g. "Display")
        result_role: Role of the result element (e.g. "text", "edit")
        window_title: Window to search in (uses focused if omitted)
    """
    provider = _require_provider()
    log.info("click_and_read_result: click %s, read name=%r role=%r", element_id, result_name, result_role)

    # Click the element
    try:
        handle = ElementID.resolve(element_id)
        click_result = provider.click_element(handle)
    except KeyError as e:
        return str(e)
    except Exception as e:
        return f"Error clicking element: {e}"

    # Brief pause for UI to update
    time.sleep(0.3)

    # Find and read the result
    try:
        elements = provider.find_elements(
            name=result_name, role=result_role, window_title=window_title
        )
        if not elements:
            return f"{click_result}\n\nNo result element found matching name={result_name!r} role={result_role!r}"

        results = [f"{click_result}\n\nResult element(s):"]
        for elem in elements:
            text = provider.get_element_text(ElementID.resolve(elem.id))
            results.append(f"  [{elem.role}] '{elem.name}': {text}")
        return "\n".join(results)
    except Exception as e:
        return f"{click_result}\n\nError reading result: {e}"


@mcp.tool()
def type_and_get_tree(
    element_id: str,
    text: str,
    window_title: Optional[str] = None,
    max_depth: int = 3,
) -> str:
    """Set a value on an element and return the updated UI tree — all in one call.

    This replaces the 2-step pattern: set_value() + get_ui_tree().
    Saves 1 LLM round-trip.

    Args:
        element_id: Element ID of the text field
        text: The text to type/set
        window_title: Window to get the updated tree from (uses focused if omitted)
        max_depth: How deep to walk the updated tree (default 3)
    """
    provider = _require_provider()
    log.info("type_and_get_tree: set %s = %r, then tree", element_id, text[:50])

    # Set the value
    try:
        handle = ElementID.resolve(element_id)
        set_result = provider.set_value(handle, text)
    except KeyError as e:
        return str(e)
    except Exception as e:
        return f"Error setting value: {e}"

    # Brief pause for UI to update
    time.sleep(0.2)

    # Get the updated tree
    try:
        tree = provider.get_ui_tree(window_title, max_depth, False)
        tree_text = tree.to_text(max_depth=max_depth)
        return f"{set_result}\n\nUpdated UI tree:\n{tree_text}"
    except Exception as e:
        return f"{set_result}\n\nCould not get updated tree: {e}"


# ===================================================================
# FALLBACK TOOLS — for apps with poor accessibility support
# ===================================================================

DANGEROUS_KEYS = {
    "alt+f4", "alt+delete", "ctrl+alt+delete", "ctrl+w", "ctrl+shift+delete",
    "super+l", "win+l",
}

_KEY_MAP = {
    "enter": Key.enter, "return": Key.enter,
    "tab": Key.tab,
    "space": Key.space,
    "backspace": Key.backspace,
    "delete": Key.delete, "del": Key.delete,
    "escape": Key.esc, "esc": Key.esc,
    "up": Key.up, "down": Key.down, "left": Key.left, "right": Key.right,
    "home": Key.home, "end": Key.end,
    "pageup": Key.page_up, "page_up": Key.page_up,
    "pagedown": Key.page_down, "page_down": Key.page_down,
    "insert": Key.insert,
    "f1": Key.f1, "f2": Key.f2, "f3": Key.f3, "f4": Key.f4,
    "f5": Key.f5, "f6": Key.f6, "f7": Key.f7, "f8": Key.f8,
    "f9": Key.f9, "f10": Key.f10, "f11": Key.f11, "f12": Key.f12,
    "ctrl": Key.ctrl, "control": Key.ctrl,
    "alt": Key.alt,
    "shift": Key.shift,
    "super": Key.cmd, "win": Key.cmd, "cmd": Key.cmd, "meta": Key.cmd,
    "capslock": Key.caps_lock, "caps_lock": Key.caps_lock,
    "numlock": Key.num_lock, "num_lock": Key.num_lock,
    "printscreen": Key.print_screen, "print_screen": Key.print_screen,
    "scrolllock": Key.scroll_lock, "scroll_lock": Key.scroll_lock,
    "pause": Key.pause,
    "menu": Key.menu,
}


def _resolve_key(name: str):
    lower = name.strip().lower()
    if lower in _KEY_MAP:
        return _KEY_MAP[lower]
    if len(name) == 1:
        return name
    raise ValueError(f"Unknown key: {name!r}")


def _capture_screenshot(
    region: Optional[dict] = None,
    max_width: int = 1280,
) -> tuple:
    with mss.mss() as sct:
        if region:
            monitor = {
                "top": region["top"],
                "left": region["left"],
                "width": region["width"],
                "height": region["height"],
            }
        else:
            monitor = sct.monitors[1]
        raw = sct.grab(monitor)
        img = PILImage.frombytes("RGB", raw.size, raw.bgra, "raw", "BGRX")

    original_w, original_h = img.width, img.height
    if img.width > max_width:
        ratio = max_width / img.width
        new_size = (max_width, int(img.height * ratio))
        img = img.resize(new_size, PILImage.LANCZOS)

    scaled_w, scaled_h = img.width, img.height
    buf = io.BytesIO()
    img.save(buf, format="JPEG", quality=60, optimize=True)
    return buf.getvalue(), original_w, original_h, scaled_w, scaled_h


@mcp.tool()
def screenshot(
    region_top: Optional[int] = None,
    region_left: Optional[int] = None,
    region_width: Optional[int] = None,
    region_height: Optional[int] = None,
    max_width: int = 1280,
) -> list:
    """Capture a screenshot. FALLBACK — prefer get_ui_tree() for understanding the UI.

    Use this only when the accessibility tree is empty or insufficient
    (e.g. games, custom-drawn UIs, image verification).

    Args:
        region_top: Top Y coordinate of capture region (optional)
        region_left: Left X coordinate (optional)
        region_width: Width (optional)
        region_height: Height (optional)
        max_width: Scale down to this width (default 1280)
    """
    region = None
    if all(v is not None for v in [region_top, region_left, region_width, region_height]):
        region = {"top": region_top, "left": region_left, "width": region_width, "height": region_height}

    try:
        jpeg_bytes, orig_w, orig_h, scaled_w, scaled_h = _capture_screenshot(region=region, max_width=max_width)
        b64 = base64.b64encode(jpeg_bytes).decode("ascii")
        scale_factor = orig_w / scaled_w if scaled_w else 1.0
        log.info("screenshot: %d bytes, %dx%d -> %dx%d", len(jpeg_bytes), orig_w, orig_h, scaled_w, scaled_h)
        info = (
            f"Screenshot: {orig_w}x{orig_h}px → {scaled_w}x{scaled_h}px "
            f"(scale {scale_factor:.2f}x). "
            f"Multiply image coords by {scale_factor:.2f} for real screen coords."
        )
        return [
            TextContent(type="text", text=info),
            ImageContent(type="image", data=b64, mimeType="image/jpeg"),
        ]
    except Exception:
        log.exception("screenshot failed")
        raise


@mcp.tool()
def click(
    x: Optional[int] = None,
    y: Optional[int] = None,
    button: str = "left",
    count: int = 1,
) -> str:
    """Click at screen coordinates. FALLBACK — prefer click_element() when possible.

    Args:
        x: X coordinate (optional — uses current position if omitted)
        y: Y coordinate (optional)
        button: 'left', 'right', or 'middle'
        count: Number of clicks (1=single, 2=double)
    """
    if x is not None and y is not None:
        mouse.position = (x, y)
    btn = {"left": Button.left, "right": Button.right, "middle": Button.middle}.get(button, Button.left)
    mouse.click(btn, count)
    pos = mouse.position
    log.info("click: %s at (%d, %d) x%d", button, pos[0], pos[1], count)
    return f"Clicked {button} at ({pos[0]}, {pos[1]})"


@mcp.tool()
def type_text(text: str, interval: float = 0.02) -> str:
    """Type text at the current cursor position. FALLBACK — prefer set_value() when possible.

    Args:
        text: The text to type
        interval: Delay between keystrokes in seconds (default 0.02)
    """
    for char in text:
        keyboard.type(char)
        if interval > 0:
            time.sleep(interval)
    log.info("type_text: %d chars", len(text))
    return f"Typed {len(text)} characters"


@mcp.tool()
def key_press(keys: str) -> str:
    """Press a key or key combination (e.g. 'ctrl+s', 'enter', 'f5').

    Dangerous combos (alt+f4, ctrl+w, etc.) require key_press_confirmed().

    Args:
        keys: Key combo separated by '+' (e.g. 'ctrl+shift+t')
    """
    normalized = keys.strip().lower().replace(" ", "")
    log.info("key_press: %r", keys)

    if normalized in DANGEROUS_KEYS:
        return (
            f"⚠️ DANGEROUS: '{keys}' — call "
            f"key_press_confirmed(keys='{keys}', confirm=True) to proceed."
        )

    parts = [p.strip() for p in keys.split("+")]
    resolved = [_resolve_key(p) for p in parts]

    if len(resolved) == 1:
        keyboard.press(resolved[0])
        keyboard.release(resolved[0])
    else:
        for mod in resolved[:-1]:
            keyboard.press(mod)
        keyboard.press(resolved[-1])
        keyboard.release(resolved[-1])
        for mod in reversed(resolved[:-1]):
            keyboard.release(mod)

    return f"Pressed: {keys}"


@mcp.tool()
def key_press_confirmed(keys: str, confirm: bool = False) -> str:
    """Execute a dangerous key combination after confirmation.

    Args:
        keys: The key combination
        confirm: Must be True to proceed
    """
    if not confirm:
        return "Cancelled — confirm must be True."

    log.info("key_press_confirmed: %r", keys)
    parts = [p.strip() for p in keys.split("+")]
    resolved = [_resolve_key(p) for p in parts]

    if len(resolved) == 1:
        keyboard.press(resolved[0])
        keyboard.release(resolved[0])
    else:
        for mod in resolved[:-1]:
            keyboard.press(mod)
        keyboard.press(resolved[-1])
        keyboard.release(resolved[-1])
        for mod in reversed(resolved[:-1]):
            keyboard.release(mod)

    return f"Executed: {keys}"


@mcp.tool()
def launch_app(name: str) -> str:
    """Launch an application by name.

    Examples: 'notepad', 'mspaint', 'calc', 'explorer'

    Args:
        name: Application name or full path
    """
    system = platform.system()
    log.info("launch_app: %r on %s", name, system)
    try:
        if system == "Windows":
            proc = subprocess.Popen(
                f'start "" {name}', shell=True,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            try:
                _, stderr = proc.communicate(timeout=5)
                if proc.returncode != 0:
                    err = stderr.decode("utf-8", errors="replace").strip()
                    return f"Failed to launch '{name}': {err or 'exit code ' + str(proc.returncode)}"
            except subprocess.TimeoutExpired:
                pass
            time.sleep(1.5)
            return f"Launched: {name}. Call get_ui_tree() to see its UI."
        elif system == "Darwin":
            proc = subprocess.Popen(["open", "-a", name], stderr=subprocess.PIPE)
            try:
                _, stderr = proc.communicate(timeout=5)
                if proc.returncode != 0:
                    return f"Failed to launch '{name}': {stderr.decode('utf-8', errors='replace').strip()}"
            except subprocess.TimeoutExpired:
                pass
            return f"Launched: {name}. Call get_ui_tree() to see its UI."
        else:
            subprocess.Popen([name])
            return f"Launched: {name}. Call get_ui_tree() to see its UI."
    except Exception as e:
        log.error("launch_app failed: %s", e)
        return f"Failed to launch '{name}': {e}"


@mcp.tool()
def drag(
    from_x: int, from_y: int, to_x: int, to_y: int,
    duration: float = 0.5, button: str = "left",
) -> str:
    """Click and drag between two points.

    Args:
        from_x: Starting X
        from_y: Starting Y
        to_x: Ending X
        to_y: Ending Y
        duration: Drag duration in seconds (default 0.5)
        button: 'left' or 'right'
    """
    btn = Button.left if button == "left" else Button.right
    mouse.position = (from_x, from_y)
    time.sleep(0.05)
    mouse.press(btn)
    time.sleep(0.05)

    steps = max(int(duration * 60), 10)
    dx = (to_x - from_x) / steps
    dy = (to_y - from_y) / steps
    for i in range(steps):
        mouse.position = (int(from_x + dx * (i + 1)), int(from_y + dy * (i + 1)))
        time.sleep(duration / steps)

    mouse.release(btn)
    log.info("drag: (%d,%d)->(%d,%d)", from_x, from_y, to_x, to_y)
    return f"Dragged from ({from_x},{from_y}) to ({to_x},{to_y})"


@mcp.tool()
def scroll(
    direction: str = "down", amount: int = 3,
    x: Optional[int] = None, y: Optional[int] = None,
) -> str:
    """Scroll the mouse wheel at coordinates. FALLBACK — prefer scroll_element().

    Args:
        direction: 'up' or 'down'
        amount: Scroll steps (default 3)
        x: X coordinate (optional)
        y: Y coordinate (optional)
    """
    if x is not None and y is not None:
        mouse.position = (x, y)
    clicks = amount if direction == "up" else -amount
    mouse.scroll(0, clicks)
    log.info("scroll: %s by %d", direction, amount)
    return f"Scrolled {direction} by {amount}"


@mcp.tool()
def move_mouse(x: int, y: int) -> str:
    """Move the mouse cursor to an absolute position.

    Args:
        x: X coordinate
        y: Y coordinate
    """
    mouse.position = (x, y)
    log.info("move_mouse: (%d, %d)", x, y)
    return f"Mouse moved to ({x}, {y})"


@mcp.tool()
def wait(milliseconds: int = 500) -> str:
    """Wait for a duration (let UI settle after actions).

    Args:
        milliseconds: How long to wait (default 500)
    """
    time.sleep(milliseconds / 1000.0)
    log.info("wait: %dms", milliseconds)
    return f"Waited {milliseconds}ms"


@mcp.tool()
def get_cursor_position() -> dict:
    """Get the current mouse cursor position.

    Returns:
        Dictionary with 'x' and 'y' keys.
    """
    pos = mouse.position
    return {"x": pos[0], "y": pos[1]}


@mcp.tool()
def get_screen_size() -> dict:
    """Get the primary monitor resolution.

    Returns:
        Dictionary with 'width' and 'height' keys.
    """
    with mss.mss() as sct:
        mon = sct.monitors[1]
        return {"width": mon["width"], "height": mon["height"]}


# ===================================================================
# Entry point
# ===================================================================

def main():
    """Run the MCP server over stdio."""
    log.info("Starting MCP stdio transport...")
    try:
        mcp.run(transport="stdio")
    except (OSError, BrokenPipeError):
        log.info("MCP server: stdio pipe closed (parent exited)")
    except Exception:
        log.exception("MCP server crashed")
        raise
    finally:
        log.info("MCP server exiting")


if __name__ == "__main__":
    main()

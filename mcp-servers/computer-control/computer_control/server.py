"""
Computer Control MCP Server

Provides tools for an AI agent to control the computer:
mouse movement, clicking, keyboard input, screenshots, and app launching.
"""

import asyncio
import base64
import io
import logging
import os
import platform
import subprocess
import sys
import time
from typing import Optional

LOG_DIR = os.path.join(os.path.expanduser("~"), ".kiro", "logs")
os.makedirs(LOG_DIR, exist_ok=True)
LOG_FILE = os.path.join(LOG_DIR, "computer-control-mcp.log")

_file_handler = logging.FileHandler(LOG_FILE, mode="a", encoding="utf-8")
_file_handler.setLevel(logging.DEBUG)
_file_handler.setFormatter(logging.Formatter("%(asctime)s [%(levelname)s] %(name)s — %(message)s"))

log = logging.getLogger("computer-control")
log.setLevel(logging.DEBUG)
log.addHandler(_file_handler)
# Don't propagate to root logger (which writes to stderr/stdout and would
# corrupt the JSON-RPC stdio transport)
log.propagate = False

log.info("=" * 60)
log.info("Computer Control MCP server starting (pid=%d)", os.getpid())

import mss
import mss.tools
from mcp.server.fastmcp import FastMCP
from PIL import Image as PILImage
from pynput.keyboard import Controller as KeyboardController
from pynput.keyboard import Key
from pynput.mouse import Button
from pynput.mouse import Controller as MouseController

# ---------------------------------------------------------------------------
# Dangerous key combos that require confirmation
# ---------------------------------------------------------------------------
DANGEROUS_KEYS = {
    "alt+f4", "alt+delete", "ctrl+alt+delete", "ctrl+w", "ctrl+shift+delete",
    "super+l", "win+l",  # lock screen
    "alt+f4",             # close window
}

# ---------------------------------------------------------------------------
# MCP Server
# ---------------------------------------------------------------------------
mcp = FastMCP(
    "computer-control",
    instructions="""\
Control the computer: move/click the mouse, type text, press keys, \
take screenshots, scroll, drag, and launch applications.

CRITICAL WORKFLOW — follow this for EVERY computer control task:

1. PLAN FIRST: Before taking any action, create a numbered task list of steps \
needed to accomplish the goal. Format it as a markdown checklist:
   ```
   ## Task Plan
   - [ ] Step 1: Open the application
   - [ ] Step 2: Wait for it to load and verify with screenshot
   - [ ] Step 3: Navigate to the right place
   ...
   ```

2. SCREENSHOT BEFORE ACTING: Always call screenshot() before performing actions \
to understand the current screen state. Never assume what's on screen.

3. VERIFY AFTER EACH STEP: After completing each step, take a screenshot to \
confirm it worked before moving to the next step. Update the checklist:
   - [ ] becomes - [x] when complete
   - Add notes if something unexpected happened

4. ADAPT THE PLAN: If the screen shows something unexpected (a dialog, a welcome \
screen, an error), stop and reassess. Update your plan before continuing.

5. BE PRECISE WITH CLICKS: When clicking UI elements, identify their exact \
coordinates from the screenshot. Don't guess positions.

6. WAIT FOR UI: After launching apps or clicking buttons, use wait() before \
screenshotting to let the UI settle. Apps often have splash screens or loading states.

7. If you don't know how to use a specific application, search the web for \
instructions before attempting to use it.

Common pitfalls to avoid:
- Don't type text before confirming the right window/field is focused
- Don't assume an app is ready immediately after launching — always verify
- Don't skip dialog boxes or welcome screens — handle them explicitly
- When an app has a welcome/start screen, dismiss it before proceeding
""",
)

mouse = MouseController()
keyboard = KeyboardController()


# ---------------------------------------------------------------------------
# Helper: capture & encode screenshot
# ---------------------------------------------------------------------------
def _capture_screenshot(
    region: Optional[dict] = None,
    max_width: int = 1280,
) -> tuple:
    """Capture the screen (or a region) and return (JPEG bytes, original_width, original_height, scaled_width, scaled_height).

    Images are scaled down to *max_width* and compressed as JPEG
    to keep the payload small enough for LLM APIs.
    """
    with mss.mss() as sct:
        if region:
            monitor = {
                "top": region["top"],
                "left": region["left"],
                "width": region["width"],
                "height": region["height"],
            }
        else:
            # Primary monitor (index 1); index 0 is the virtual "all monitors" screen
            monitor = sct.monitors[1]

        raw = sct.grab(monitor)
        img = PILImage.frombytes("RGB", raw.size, raw.bgra, "raw", "BGRX")

    original_w, original_h = img.width, img.height

    # Scale down if wider than max_width
    if img.width > max_width:
        ratio = max_width / img.width
        new_size = (max_width, int(img.height * ratio))
        img = img.resize(new_size, PILImage.LANCZOS)

    scaled_w, scaled_h = img.width, img.height

    buf = io.BytesIO()
    img.save(buf, format="JPEG", quality=60, optimize=True)
    return buf.getvalue(), original_w, original_h, scaled_w, scaled_h


# ---------------------------------------------------------------------------
# Helper: parse key names to pynput Key objects
# ---------------------------------------------------------------------------
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
    """Convert a key name string to a pynput key."""
    lower = name.strip().lower()
    if lower in _KEY_MAP:
        return _KEY_MAP[lower]
    if len(name) == 1:
        return name
    raise ValueError(f"Unknown key: {name!r}")


from mcp.types import ImageContent, TextContent


# ===================================================================
# TOOLS
# ===================================================================


@mcp.tool()
def screenshot(
    region_top: Optional[int] = None,
    region_left: Optional[int] = None,
    region_width: Optional[int] = None,
    region_height: Optional[int] = None,
    max_width: int = 1280,
) -> list:
    """Capture a screenshot of the screen. This is your 'eyes' — call this
    to see what's currently on screen before and after performing actions.

    Returns the screenshot as an embedded image.

    Args:
        region_top: Top Y coordinate of capture region (optional, captures full screen if omitted)
        region_left: Left X coordinate of capture region (optional)
        region_width: Width of capture region (optional)
        region_height: Height of capture region (optional)
        max_width: Scale image down to this width to save tokens (default 1280)
    """
    region = None
    if all(v is not None for v in [region_top, region_left, region_width, region_height]):
        region = {
            "top": region_top,
            "left": region_left,
            "width": region_width,
            "height": region_height,
        }

    try:
        jpeg_bytes, orig_w, orig_h, scaled_w, scaled_h = _capture_screenshot(region=region, max_width=max_width)
        b64 = base64.b64encode(jpeg_bytes).decode("ascii")

        scale_factor = orig_w / scaled_w if scaled_w else 1.0

        log.info("screenshot: %d bytes, %dx%d -> %dx%d (scale=%.2f), region=%s",
                 len(jpeg_bytes), orig_w, orig_h, scaled_w, scaled_h, scale_factor, region)

        info = (
            f"Screenshot captured. "
            f"Screen: {orig_w}x{orig_h}px. "
            f"Image: {scaled_w}x{scaled_h}px (scale factor: {scale_factor:.2f}x). "
            f"IMPORTANT: Multiply image coordinates by {scale_factor:.2f} to get real screen coordinates for clicking."
        )

        return [
            TextContent(type="text", text=info),
            ImageContent(type="image", data=b64, mimeType="image/jpeg"),
        ]
    except Exception:
        log.exception("screenshot: FAILED")
        raise


# ---------------------------------------------------------------------------
# Window-specific screenshot (Windows only, graceful fallback on other OS)
# ---------------------------------------------------------------------------
def _find_windows(title_pattern: str) -> list:
    """Find windows whose title contains the given pattern (case-insensitive)."""
    if platform.system() != "Windows":
        return []
    try:
        import ctypes
        from ctypes import wintypes

        user32 = ctypes.windll.user32
        results = []

        def enum_callback(hwnd, _):
            if not user32.IsWindowVisible(hwnd):
                return True
            length = user32.GetWindowTextLengthW(hwnd)
            if length == 0:
                return True
            buf = ctypes.create_unicode_buffer(length + 1)
            user32.GetWindowTextW(hwnd, buf, length + 1)
            title = buf.value
            if title_pattern.lower() in title.lower():
                rect = wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                results.append({
                    "hwnd": hwnd,
                    "title": title,
                    "left": rect.left,
                    "top": rect.top,
                    "width": rect.right - rect.left,
                    "height": rect.bottom - rect.top,
                })
            return True

        WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)
        user32.EnumWindows(WNDENUMPROC(enum_callback), 0)
        return results
    except Exception as e:
        log.error("_find_windows failed: %s", e)
        return []


@mcp.tool()
def list_windows(title_pattern: str = "") -> list:
    """List visible windows, optionally filtered by title pattern.

    Args:
        title_pattern: Filter windows whose title contains this text (case-insensitive). Empty = all windows.

    Returns:
        List of {title, left, top, width, height} for each matching window.
    """
    windows = _find_windows(title_pattern) if title_pattern else _find_windows("")
    # Return without hwnd (not useful to the agent)
    result = [{"title": w["title"], "left": w["left"], "top": w["top"],
               "width": w["width"], "height": w["height"]} for w in windows
              if w["width"] > 50 and w["height"] > 50]  # filter tiny/hidden windows
    log.info("list_windows: pattern=%r, found %d windows", title_pattern, len(result))
    return result


@mcp.tool()
def screenshot_window(
    title_pattern: str,
    max_width: int = 1280,
) -> list:
    """Capture a screenshot of a specific window by its title.

    This is more precise than a full-screen screenshot — it captures just the
    window at higher resolution, making UI elements easier to read.

    Args:
        title_pattern: Text to match in the window title (case-insensitive). E.g. 'Word', 'Notepad'
        max_width: Scale image down to this width (default 1280)

    Returns:
        Screenshot of the matched window, or an error if not found.
    """
    windows = _find_windows(title_pattern)
    if not windows:
        log.warning("screenshot_window: no window matching %r", title_pattern)
        return f"No window found matching '{title_pattern}'. Use list_windows() to see available windows."

    # Pick the largest matching window (most likely the main one)
    win = max(windows, key=lambda w: w["width"] * w["height"])
    log.info("screenshot_window: capturing '%s' at (%d,%d) %dx%d",
             win["title"], win["left"], win["top"], win["width"], win["height"])

    region = {
        "top": win["top"],
        "left": win["left"],
        "width": win["width"],
        "height": win["height"],
    }

    try:
        jpeg_bytes, orig_w, orig_h, scaled_w, scaled_h = _capture_screenshot(
            region=region, max_width=max_width
        )
        b64 = base64.b64encode(jpeg_bytes).decode("ascii")
        scale_factor = orig_w / scaled_w if scaled_w else 1.0

        info = (
            f"Window '{win['title']}' captured. "
            f"Window position: ({win['left']}, {win['top']}), size: {orig_w}x{orig_h}px. "
            f"Image: {scaled_w}x{scaled_h}px (scale factor: {scale_factor:.2f}x). "
            f"IMPORTANT: To click inside this window, multiply image coordinates by {scale_factor:.2f} "
            f"then add the window offset ({win['left']}, {win['top']})."
        )

        return [
            TextContent(type="text", text=info),
            ImageContent(type="image", data=b64, mimeType="image/jpeg"),
        ]
    except Exception:
        log.exception("screenshot_window: FAILED for '%s'", win["title"])
        raise


@mcp.tool()
def get_screen_size() -> dict:
    """Get the screen resolution of the primary monitor.

    Returns:
        Dictionary with 'width' and 'height' keys.
    """
    with mss.mss() as sct:
        mon = sct.monitors[1]  # primary monitor
        log.info("get_screen_size: %dx%d", mon["width"], mon["height"])
        return {"width": mon["width"], "height": mon["height"]}


@mcp.tool()
def move_mouse(x: int, y: int) -> str:
    """Move the mouse cursor to an absolute screen position.

    Args:
        x: X coordinate (pixels from left edge)
        y: Y coordinate (pixels from top edge)
    """
    mouse.position = (x, y)
    log.info("move_mouse: (%d, %d)", x, y)
    return f"Mouse moved to ({x}, {y})"


@mcp.tool()
def click(
    x: Optional[int] = None,
    y: Optional[int] = None,
    button: str = "left",
    count: int = 1,
) -> str:
    """Click the mouse at the given position (or current position if omitted).

    Args:
        x: X coordinate to click at (optional — uses current position if omitted)
        y: Y coordinate to click at (optional)
        button: Mouse button — 'left', 'right', or 'middle'
        count: Number of clicks (1 = single, 2 = double)
    """
    if x is not None and y is not None:
        mouse.position = (x, y)

    btn = {"left": Button.left, "right": Button.right, "middle": Button.middle}.get(
        button, Button.left
    )
    mouse.click(btn, count)

    pos = mouse.position
    click_type = "Double-clicked" if count == 2 else "Clicked"
    log.info("click: %s %s at (%d, %d), count=%d", click_type, button, pos[0], pos[1], count)
    return f"{click_type} {button} at ({pos[0]}, {pos[1]})"


@mcp.tool()
def double_click(x: Optional[int] = None, y: Optional[int] = None) -> str:
    """Double-click the left mouse button at the given position.

    Args:
        x: X coordinate (optional — uses current position if omitted)
        y: Y coordinate (optional)
    """
    if x is not None and y is not None:
        mouse.position = (x, y)
    mouse.click(Button.left, 2)
    pos = mouse.position
    log.info("double_click: (%d, %d)", pos[0], pos[1])
    return f"Double-clicked at ({pos[0]}, {pos[1]})"


@mcp.tool()
def right_click(x: Optional[int] = None, y: Optional[int] = None) -> str:
    """Right-click at the given position.

    Args:
        x: X coordinate (optional — uses current position if omitted)
        y: Y coordinate (optional)
    """
    if x is not None and y is not None:
        mouse.position = (x, y)
    mouse.click(Button.right, 1)
    pos = mouse.position
    log.info("right_click: (%d, %d)", pos[0], pos[1])
    return f"Right-clicked at ({pos[0]}, {pos[1]})"


@mcp.tool()
def drag(
    from_x: int,
    from_y: int,
    to_x: int,
    to_y: int,
    duration: float = 0.5,
    button: str = "left",
) -> str:
    """Click and drag from one position to another.

    Args:
        from_x: Starting X coordinate
        from_y: Starting Y coordinate
        to_x: Ending X coordinate
        to_y: Ending Y coordinate
        duration: How long the drag takes in seconds (default 0.5)
        button: Mouse button to hold during drag — 'left' or 'right'
    """
    btn = Button.left if button == "left" else Button.right

    mouse.position = (from_x, from_y)
    time.sleep(0.05)  # small settle time

    mouse.press(btn)
    time.sleep(0.05)

    # Interpolate movement for smooth drag
    steps = max(int(duration * 60), 10)  # ~60 steps per second
    dx = (to_x - from_x) / steps
    dy = (to_y - from_y) / steps
    for i in range(steps):
        mouse.position = (int(from_x + dx * (i + 1)), int(from_y + dy * (i + 1)))
        time.sleep(duration / steps)

    mouse.release(btn)
    log.info("drag: (%d,%d) -> (%d,%d), duration=%.2fs, button=%s", from_x, from_y, to_x, to_y, duration, button)
    return f"Dragged from ({from_x}, {from_y}) to ({to_x}, {to_y})"


@mcp.tool()
def scroll(
    direction: str = "down",
    amount: int = 3,
    x: Optional[int] = None,
    y: Optional[int] = None,
) -> str:
    """Scroll the mouse wheel.

    Args:
        direction: 'up' or 'down'
        amount: Number of scroll steps (default 3)
        x: X coordinate to scroll at (optional — uses current position)
        y: Y coordinate to scroll at (optional)
    """
    if x is not None and y is not None:
        mouse.position = (x, y)

    clicks = amount if direction == "up" else -amount
    mouse.scroll(0, clicks)
    log.info("scroll: %s by %d at current pos", direction, amount)
    return f"Scrolled {direction} by {amount} steps"


@mcp.tool()
def type_text(text: str, interval: float = 0.02) -> str:
    """Type a string of text at the current cursor position.

    This simulates individual key presses. For special keys or combos,
    use key_press() instead.

    Args:
        text: The text to type
        interval: Delay between keystrokes in seconds (default 0.02)
    """
    for char in text:
        keyboard.type(char)
        if interval > 0:
            time.sleep(interval)
    log.info("type_text: %d chars, interval=%.3f, text=%r", len(text), interval, text[:100])
    return f"Typed {len(text)} characters"


@mcp.tool()
def key_press(keys: str) -> str:
    """Press a key or key combination.

    Use '+' to combine modifier keys. Examples:
    - 'enter'
    - 'ctrl+s'
    - 'ctrl+shift+t'
    - 'alt+f4'  (DANGEROUS — will close the active window)
    - 'ctrl+a'
    - 'f5'

    DANGEROUS COMBINATIONS that will request confirmation:
    alt+f4, ctrl+alt+delete, ctrl+w, super+l/win+l

    Args:
        keys: Key name or combination separated by '+' (e.g. 'ctrl+s')
    """
    normalized = keys.strip().lower().replace(" ", "")

    log.info("key_press: %r (normalized=%r)", keys, normalized)

    # Safety check for dangerous combos
    if normalized in DANGEROUS_KEYS:
        return (
            f"⚠️ DANGEROUS ACTION: '{keys}' could close windows or lock the screen. "
            f"Please confirm you want to execute this by calling "
            f"key_press_confirmed(keys='{keys}', confirm=True)"
        )

    parts = [p.strip() for p in keys.split("+")]
    resolved = [_resolve_key(p) for p in parts]

    # Press modifiers, press the final key, then release in reverse
    if len(resolved) == 1:
        keyboard.press(resolved[0])
        keyboard.release(resolved[0])
    else:
        # Hold modifiers, tap the last key
        for mod in resolved[:-1]:
            keyboard.press(mod)
        keyboard.press(resolved[-1])
        keyboard.release(resolved[-1])
        for mod in reversed(resolved[:-1]):
            keyboard.release(mod)

    return f"Pressed: {keys}"


@mcp.tool()
def key_press_confirmed(keys: str, confirm: bool = False) -> str:
    """Execute a dangerous key combination after explicit confirmation.

    This is only needed for key combos flagged as dangerous by key_press().
    The agent must set confirm=True to proceed.

    Args:
        keys: The key combination to execute
        confirm: Must be True to proceed
    """
    if not confirm:
        log.warning("key_press_confirmed: REJECTED (confirm=False) for %r", keys)
        return "Action cancelled — confirm must be True to execute dangerous key combos."

    log.info("key_press_confirmed: EXECUTING dangerous combo %r", keys)

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

    return f"Executed confirmed dangerous action: {keys}"


@mcp.tool()
def launch_app(name: str) -> str:
    """Launch an application by name.

    On Windows, tries the 'start' command (handles Start Menu shortcuts and
    App Paths registry). Verifies the app actually appeared by checking for
    a new window after a short delay.
    Examples: 'notepad', 'mspaint', 'calc', 'explorer', 'winword', 'excel'

    Args:
        name: Application name or full path
    """
    import shutil

    system = platform.system()
    log.info("launch_app: %r on %s", name, system)
    try:
        if system == "Windows":
            # Use 'start' command which resolves Start Menu shortcuts and App Paths
            proc = subprocess.Popen(
                f'start "" "{name}"',
                shell=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            try:
                _, stderr = proc.communicate(timeout=5)
                if proc.returncode != 0:
                    err = stderr.decode("utf-8", errors="replace").strip()
                    log.warning("launch_app: 'start' failed (rc=%d): %s", proc.returncode, err)
                    return f"Failed to launch '{name}': {err or 'exit code ' + str(proc.returncode)}"
            except subprocess.TimeoutExpired:
                pass

            # Give the app a moment to start, then take a screenshot
            # so the agent can verify it opened
            time.sleep(1.5)
            log.info("launch_app: 'start' command completed for %r", name)
            return (
                f"Launched: {name}. "
                f"Call screenshot() to verify the application opened successfully."
            )
        elif system == "Darwin":
            proc = subprocess.Popen(["open", "-a", name], stderr=subprocess.PIPE)
            try:
                _, stderr = proc.communicate(timeout=5)
                if proc.returncode != 0:
                    err = stderr.decode("utf-8", errors="replace").strip()
                    return f"Failed to launch '{name}': {err}"
            except subprocess.TimeoutExpired:
                pass
            return f"Launched: {name}. Call screenshot() to verify."
        else:
            subprocess.Popen([name])
            return f"Launched: {name}. Call screenshot() to verify."
    except Exception as e:
        log.error("launch_app: exception launching %r: %s", name, e)
        return f"Failed to launch '{name}': {e}"


@mcp.tool()
def wait(milliseconds: int = 500) -> str:
    """Wait for a specified duration. Useful between actions to let
    the UI settle (e.g. after launching an app, before taking a screenshot).

    Args:
        milliseconds: How long to wait in milliseconds (default 500)
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
    log.info("get_cursor_position: (%d, %d)", pos[0], pos[1])
    return {"x": pos[0], "y": pos[1]}


# ===================================================================
# Entry point
# ===================================================================

def main():
    """Run the MCP server over stdio."""
    log.info("Starting MCP stdio transport...")
    try:
        mcp.run(transport="stdio")
    except (OSError, BrokenPipeError):
        # Parent process closed the pipe — normal shutdown
        log.info("MCP server: stdio pipe closed (parent exited)")
    except Exception:
        log.exception("MCP server crashed")
        raise
    finally:
        log.info("MCP server exiting")


if __name__ == "__main__":
    main()

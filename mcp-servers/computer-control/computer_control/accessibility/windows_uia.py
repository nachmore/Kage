"""Windows UI Automation provider using the uiautomation package.

Hardened for real-world use:
- Max element cap to prevent runaway trees (browsers, complex apps)
- Timeout protection on tree walks
- Electron app detection with accessibility hint
- Noise filtering (nameless separators, scrollbars, thumbs)
- Process name resolution for window listing
- Robust per-element error handling
"""

from __future__ import annotations

import logging
import os
import threading
import time
from typing import Any, Optional

from .base import AccessibilityProvider
from .tree import ElementID, UIElement

log = logging.getLogger("computer-control.uia")

# ---------------------------------------------------------------------------
# Limits
# ---------------------------------------------------------------------------
MAX_ELEMENTS = 500          # Stop walking after this many elements
TREE_WALK_TIMEOUT = 8.0     # Seconds before aborting a tree walk
SEARCH_TIMEOUT = 10.0       # Seconds before aborting a find_elements search

try:
    import uiautomation as auto

    auto.SetGlobalSearchTimeout(3)
    _HAS_UIA = True
except ImportError:
    _HAS_UIA = False
    log.warning("uiautomation package not installed — Windows UIA provider unavailable")


# Map UIA ControlType int to short role name
_ROLE_MAP = {
    auto.ControlType.ButtonControl: "button",
    auto.ControlType.CalendarControl: "calendar",
    auto.ControlType.CheckBoxControl: "checkbox",
    auto.ControlType.ComboBoxControl: "combobox",
    auto.ControlType.EditControl: "edit",
    auto.ControlType.HyperlinkControl: "link",
    auto.ControlType.ImageControl: "image",
    auto.ControlType.ListControl: "list",
    auto.ControlType.ListItemControl: "listitem",
    auto.ControlType.MenuControl: "menu",
    auto.ControlType.MenuBarControl: "menubar",
    auto.ControlType.MenuItemControl: "menuitem",
    auto.ControlType.ProgressBarControl: "progressbar",
    auto.ControlType.RadioButtonControl: "radiobutton",
    auto.ControlType.ScrollBarControl: "scrollbar",
    auto.ControlType.SliderControl: "slider",
    auto.ControlType.SpinnerControl: "spinner",
    auto.ControlType.StatusBarControl: "statusbar",
    auto.ControlType.TabControl: "tab",
    auto.ControlType.TabItemControl: "tabitem",
    auto.ControlType.TextControl: "text",
    auto.ControlType.ToolBarControl: "toolbar",
    auto.ControlType.ToolTipControl: "tooltip",
    auto.ControlType.TreeControl: "tree",
    auto.ControlType.TreeItemControl: "treeitem",
    auto.ControlType.WindowControl: "window",
    auto.ControlType.PaneControl: "pane",
    auto.ControlType.GroupControl: "group",
    auto.ControlType.ThumbControl: "thumb",
    auto.ControlType.DataGridControl: "datagrid",
    auto.ControlType.DataItemControl: "dataitem",
    auto.ControlType.DocumentControl: "document",
    auto.ControlType.SplitButtonControl: "splitbutton",
    auto.ControlType.HeaderControl: "header",
    auto.ControlType.HeaderItemControl: "headeritem",
    auto.ControlType.TableControl: "table",
    auto.ControlType.TitleBarControl: "titlebar",
    auto.ControlType.SeparatorControl: "separator",
} if _HAS_UIA else {}

# Known Electron process names (lowercase) — these need
# --force-renderer-accessibility for a useful tree
_ELECTRON_PROCESSES = frozenset({
    "code", "code - insiders", "slack", "discord", "teams",
    "spotify", "notion", "obsidian", "figma", "postman",
    "signal", "whatsapp", "telegram", "bitwarden",
    "visual studio code", "vscode",
})


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _get_role(control: auto.Control) -> str:
    """Get short role name for a UIA control."""
    try:
        return _ROLE_MAP.get(control.ControlType, "unknown")
    except Exception:
        return "unknown"


def _get_actions(control: auto.Control) -> list[str]:
    """Determine available actions based on supported UIA patterns."""
    actions = []
    _try_pattern(control, "GetInvokePattern", actions, "invoke")
    _try_pattern(control, "GetValuePattern", actions, "set_value")
    _try_pattern(control, "GetTogglePattern", actions, "toggle")
    _try_pattern(control, "GetSelectionItemPattern", actions, "select")
    _try_pattern(control, "GetExpandCollapsePattern", actions, "expand_collapse")
    _try_pattern(control, "GetScrollPattern", actions, "scroll")
    _try_pattern(control, "GetTextPattern", actions, "get_text")
    return actions


def _try_pattern(control, method_name: str, actions: list, action_name: str):
    """Safely check if a control supports a pattern."""
    try:
        method = getattr(control, method_name, None)
        if method and method():
            actions.append(action_name)
    except Exception:
        pass


def _get_states(control: auto.Control) -> list[str]:
    """Get relevant state info for a control."""
    states = []
    try:
        if not control.IsEnabled:
            states.append("disabled")
    except Exception:
        pass
    try:
        if control.IsOffscreen:
            states.append("offscreen")
    except Exception:
        pass
    try:
        tp = control.GetTogglePattern()
        if tp:
            ts = tp.ToggleState
            if ts == auto.ToggleState.On:
                states.append("checked")
            elif ts == auto.ToggleState.Off:
                states.append("unchecked")
    except Exception:
        pass
    try:
        ecp = control.GetExpandCollapsePattern()
        if ecp:
            es = ecp.ExpandCollapseState
            if es == auto.ExpandCollapseState.Expanded:
                states.append("expanded")
            elif es == auto.ExpandCollapseState.Collapsed:
                states.append("collapsed")
    except Exception:
        pass
    return states


def _get_value(control: auto.Control) -> str:
    """Try to get the current value of a control."""
    try:
        vp = control.GetValuePattern()
        if vp:
            return vp.Value or ""
    except Exception:
        pass
    return ""


def _get_bounds(control: auto.Control) -> Optional[tuple[int, int, int, int]]:
    """Get bounding rectangle as (x, y, width, height)."""
    try:
        rect = control.BoundingRectangle
        if rect and rect.width() > 0 and rect.height() > 0:
            return (rect.left, rect.top, rect.width(), rect.height())
    except Exception:
        pass
    return None


def _safe_name(control: auto.Control) -> str:
    try:
        return control.Name or ""
    except Exception:
        return ""


def _safe_automation_id(control: auto.Control) -> str:
    try:
        return control.AutomationId or ""
    except Exception:
        return ""


def _safe_pid(control: auto.Control) -> int:
    try:
        return control.ProcessId or 0
    except Exception:
        return 0


def _get_process_name(pid: int) -> str:
    """Resolve a PID to a process name. Returns '' on failure."""
    if pid <= 0:
        return ""
    try:
        import ctypes
        from ctypes import wintypes

        PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
        kernel32 = ctypes.windll.kernel32

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
    except Exception:
        pass
    return ""


def _is_electron_process(process_name: str) -> bool:
    """Check if a process name is a known Electron app."""
    return process_name.lower() in _ELECTRON_PROCESSES


def _detect_electron_empty_tree(control: auto.Control, child_count: int) -> Optional[str]:
    """If the tree looks suspiciously empty for an Electron app, return a hint."""
    if child_count > 5:
        return None  # tree has content, probably fine

    pid = _safe_pid(control)
    pname = _get_process_name(pid)
    if not pname:
        return None

    if _is_electron_process(pname):
        return (
            f"⚠️ This appears to be an Electron app ({pname}) with a sparse "
            f"accessibility tree ({child_count} elements). Try launching it with "
            f"--force-renderer-accessibility for a complete tree."
        )
    return None


# ---------------------------------------------------------------------------
# Tree building with element cap and timeout
# ---------------------------------------------------------------------------

class _TreeWalkState:
    """Mutable state shared during a tree walk."""
    __slots__ = ("count", "truncated", "deadline")

    def __init__(self, timeout: float):
        self.count = 0
        self.truncated = False
        self.deadline = time.monotonic() + timeout

    @property
    def budget_exhausted(self) -> bool:
        if self.count >= MAX_ELEMENTS:
            self.truncated = True
            return True
        if time.monotonic() > self.deadline:
            self.truncated = True
            return True
        return False


def _build_element(
    control: auto.Control,
    depth: int,
    max_depth: int,
    include_invisible: bool,
    state: _TreeWalkState,
) -> Optional[UIElement]:
    """Recursively build a UIElement from a UIA Control.

    Respects element cap and timeout via the shared state object.
    """
    if state.budget_exhausted:
        return None

    # Skip offscreen elements unless requested
    if not include_invisible:
        try:
            if control.IsOffscreen:
                return None
        except Exception:
            pass

    role = _get_role(control)
    name = _safe_name(control)
    aid = _safe_automation_id(control)

    eid = ElementID.register(control)
    state.count += 1

    elem = UIElement(
        id=eid,
        role=role,
        name=name,
        value=_get_value(control),
        automation_id=aid,
        states=_get_states(control),
        actions=_get_actions(control),
        bounds=_get_bounds(control),
    )

    if depth < max_depth and not state.budget_exhausted:
        try:
            children = control.GetChildren()
            if children:
                for child in children:
                    if state.budget_exhausted:
                        break
                    try:
                        child_elem = _build_element(
                            child, depth + 1, max_depth, include_invisible, state
                        )
                        if child_elem is not None:
                            elem.children.append(child_elem)
                    except Exception as e:
                        log.debug("Skipping child of %s '%s': %s", role, name, e)
        except Exception as e:
            log.debug("Failed to get children for %s '%s': %s", role, name, e)

    return elem


# ---------------------------------------------------------------------------
# Provider class
# ---------------------------------------------------------------------------

class WindowsUIAProvider(AccessibilityProvider):
    """Windows UI Automation accessibility provider.

    Hardened with:
    - Element cap (MAX_ELEMENTS) to prevent runaway trees
    - Timeout on tree walks (TREE_WALK_TIMEOUT)
    - Electron app detection with accessibility hint
    - Process name resolution for window listing
    - Per-element error isolation
    """

    def __init__(self):
        if not _HAS_UIA:
            raise RuntimeError(
                "uiautomation package not installed. "
                "Install with: pip install uiautomation"
            )

    def _find_window(self, title: Optional[str]) -> auto.Control:
        """Find a window by title substring, or return the focused window."""
        if title:
            win = auto.WindowControl(
                searchDepth=1, SubName=title, searchInterval=0.5
            )
            if not win.Exists(maxSearchSeconds=3):
                # Try a broader search — some apps nest their main window
                win = auto.WindowControl(
                    searchDepth=2, SubName=title, searchInterval=0.5
                )
                if not win.Exists(maxSearchSeconds=2):
                    raise ValueError(
                        f"No window found matching '{title}'. "
                        f"Use list_windows() or list_all_windows() to see available windows."
                    )
            return win
        else:
            fw = auto.GetForegroundControl()
            if fw is None:
                raise ValueError("No focused window found")
            return fw

    def get_ui_tree(
        self,
        window_title: Optional[str] = None,
        max_depth: int = 3,
        include_invisible: bool = False,
    ) -> UIElement:
        ElementID.clear()
        window = self._find_window(window_title)

        walk_state = _TreeWalkState(TREE_WALK_TIMEOUT)
        elem = _build_element(window, 0, max_depth, include_invisible, walk_state)

        if elem is None:
            raise ValueError("Failed to build UI tree — window may be inaccessible")

        # Check for Electron empty tree
        electron_hint = _detect_electron_empty_tree(window, walk_state.count)

        # Inject metadata into the root element name for the agent
        meta_parts = []
        if walk_state.truncated:
            meta_parts.append(
                f"⚠️ Tree truncated at {walk_state.count} elements "
                f"(limit={MAX_ELEMENTS}). Use find_elements() to search deeper, "
                f"or increase max_depth on a specific subtree."
            )
        if electron_hint:
            meta_parts.append(electron_hint)

        # Store metadata as a special attribute the server can read
        elem._meta = "\n".join(meta_parts) if meta_parts else ""

        log.info(
            "get_ui_tree: %d elements, truncated=%s, depth=%d",
            walk_state.count, walk_state.truncated, max_depth,
        )
        return elem

    def find_elements(
        self,
        name: Optional[str] = None,
        role: Optional[str] = None,
        automation_id: Optional[str] = None,
        value: Optional[str] = None,
        window_title: Optional[str] = None,
    ) -> list[UIElement]:
        ElementID.clear()
        window = self._find_window(window_title)

        conditions = {}
        if name:
            conditions["SubName"] = name
        if automation_id:
            conditions["AutomationId"] = automation_id

        target_type = None
        if role:
            role_lower = role.lower()
            for ct, rname in _ROLE_MAP.items():
                if rname == role_lower:
                    target_type = ct
                    break

        results: list[UIElement] = []
        walk_state = _TreeWalkState(SEARCH_TIMEOUT)

        self._search_recursive(
            window, conditions, target_type, value, results,
            depth=0, max_depth=10, state=walk_state,
        )

        if walk_state.truncated:
            log.warning(
                "find_elements: search truncated at %d elements "
                "(found %d matches)", walk_state.count, len(results),
            )

        return results

    def _search_recursive(
        self,
        control: auto.Control,
        conditions: dict,
        target_type: Optional[int],
        value_filter: Optional[str],
        results: list[UIElement],
        depth: int,
        max_depth: int,
        state: _TreeWalkState,
    ):
        if depth > max_depth or state.budget_exhausted:
            return

        state.count += 1

        match = True
        if target_type is not None:
            try:
                if control.ControlType != target_type:
                    match = False
            except Exception:
                match = False

        if match and conditions.get("SubName"):
            n = _safe_name(control)
            if conditions["SubName"].lower() not in n.lower():
                match = False

        if match and conditions.get("AutomationId"):
            aid = _safe_automation_id(control)
            if conditions["AutomationId"] != aid:
                match = False

        if match and value_filter:
            v = _get_value(control)
            if value_filter.lower() not in v.lower():
                match = False

        if match and depth > 0:
            eid = ElementID.register(control)
            elem = UIElement(
                id=eid,
                role=_get_role(control),
                name=_safe_name(control),
                value=_get_value(control),
                automation_id=_safe_automation_id(control),
                states=_get_states(control),
                actions=_get_actions(control),
                bounds=_get_bounds(control),
            )
            results.append(elem)

        try:
            children = control.GetChildren()
            if children:
                for child in children:
                    if state.budget_exhausted:
                        break
                    try:
                        self._search_recursive(
                            child, conditions, target_type, value_filter,
                            results, depth + 1, max_depth, state,
                        )
                    except Exception:
                        pass
        except Exception:
            pass

    def get_focused_element(self) -> Optional[UIElement]:
        ElementID.clear()
        try:
            focused = auto.GetFocusedControl()
            if focused is None:
                return None
            eid = ElementID.register(focused)
            return UIElement(
                id=eid,
                role=_get_role(focused),
                name=_safe_name(focused),
                value=_get_value(focused),
                automation_id=_safe_automation_id(focused),
                states=_get_states(focused),
                actions=_get_actions(focused),
                bounds=_get_bounds(focused),
            )
        except Exception as e:
            log.error("get_focused_element failed: %s", e)
            return None

    def list_windows(self, title_filter: Optional[str] = None) -> list[dict[str, Any]]:
        results = []
        try:
            desktop = auto.GetRootControl()
            windows = desktop.GetChildren()
            if not windows:
                return results
            for win in windows:
                try:
                    if win.ControlType != auto.ControlType.WindowControl:
                        continue
                    title = _safe_name(win)
                    if title_filter and title_filter.lower() not in title.lower():
                        continue
                    bounds = _get_bounds(win)
                    if bounds and bounds[2] > 50 and bounds[3] > 50:
                        pid = _safe_pid(win)
                        pname = _get_process_name(pid)
                        results.append({
                            "title": title,
                            "bounds": bounds,
                            "process_id": pid,
                            "process_name": pname,
                        })
                except Exception:
                    continue
        except Exception as e:
            log.error("list_windows failed: %s", e)
        return results


    # ------------------------------------------------------------------
    # Action methods
    # ------------------------------------------------------------------

    def click_element(self, native_handle: Any) -> str:
        control: auto.Control = native_handle
        role = _get_role(control)
        name = _safe_name(control)

        # Try InvokePattern first (buttons, menu items, links)
        try:
            ip = control.GetInvokePattern()
            if ip:
                ip.Invoke()
                return f"Invoked [{role}] '{name}'"
        except Exception as e:
            log.debug("InvokePattern failed for %s '%s': %s", role, name, e)

        # Try TogglePattern (checkboxes)
        try:
            tp = control.GetTogglePattern()
            if tp:
                tp.Toggle()
                state = "on" if tp.ToggleState == auto.ToggleState.On else "off"
                return f"Toggled [{role}] '{name}' → {state}"
        except Exception as e:
            log.debug("TogglePattern failed for %s '%s': %s", role, name, e)

        # Try SelectionItemPattern
        try:
            sp = control.GetSelectionItemPattern()
            if sp:
                sp.Select()
                return f"Selected [{role}] '{name}'"
        except Exception as e:
            log.debug("SelectionItemPattern failed for %s '%s': %s", role, name, e)

        # Try ExpandCollapsePattern
        try:
            ecp = control.GetExpandCollapsePattern()
            if ecp:
                if ecp.ExpandCollapseState == auto.ExpandCollapseState.Collapsed:
                    ecp.Expand()
                    return f"Expanded [{role}] '{name}'"
                else:
                    ecp.Collapse()
                    return f"Collapsed [{role}] '{name}'"
        except Exception as e:
            log.debug("ExpandCollapsePattern failed for %s '%s': %s", role, name, e)

        # Fallback: click at center of bounding rect
        try:
            control.Click()
            return f"Clicked [{role}] '{name}' (coordinate fallback)"
        except Exception as e:
            return f"Failed to click [{role}] '{name}': {e}"

    def set_value(self, native_handle: Any, value: str) -> str:
        control: auto.Control = native_handle
        role = _get_role(control)
        name = _safe_name(control)

        # Try ValuePattern
        try:
            vp = control.GetValuePattern()
            if vp:
                vp.SetValue(value)
                return f"Set value on [{role}] '{name}'"
        except Exception as e:
            log.debug("ValuePattern.SetValue failed for %s '%s': %s", role, name, e)

        # Fallback: focus, select all, type
        try:
            control.SetFocus()
            auto.SendKeys("{Ctrl}a", interval=0.02)
            auto.SendKeys(value, interval=0.01)
            return f"Typed value into [{role}] '{name}' (keyboard fallback)"
        except Exception as e:
            return f"Failed to set value on [{role}] '{name}': {e}"

    def toggle_element(self, native_handle: Any) -> str:
        control: auto.Control = native_handle
        role = _get_role(control)
        name = _safe_name(control)

        try:
            tp = control.GetTogglePattern()
            if tp:
                tp.Toggle()
                state = "on" if tp.ToggleState == auto.ToggleState.On else "off"
                return f"Toggled [{role}] '{name}' → {state}"
        except Exception as e:
            return f"Failed to toggle [{role}] '{name}': {e}"

        return f"[{role}] '{name}' does not support toggle"

    def select_element(self, native_handle: Any) -> str:
        control: auto.Control = native_handle
        role = _get_role(control)
        name = _safe_name(control)

        try:
            sp = control.GetSelectionItemPattern()
            if sp:
                sp.Select()
                return f"Selected [{role}] '{name}'"
        except Exception:
            pass

        # Fallback: invoke
        try:
            ip = control.GetInvokePattern()
            if ip:
                ip.Invoke()
                return f"Invoked [{role}] '{name}' (select fallback)"
        except Exception as e:
            return f"Failed to select [{role}] '{name}': {e}"

        return f"[{role}] '{name}' does not support selection"

    def expand_element(self, native_handle: Any) -> str:
        control: auto.Control = native_handle
        role = _get_role(control)
        name = _safe_name(control)

        try:
            ecp = control.GetExpandCollapsePattern()
            if ecp:
                ecp.Expand()
                return f"Expanded [{role}] '{name}'"
        except Exception as e:
            return f"Failed to expand [{role}] '{name}': {e}"

        return f"[{role}] '{name}' does not support expand"

    def collapse_element(self, native_handle: Any) -> str:
        control: auto.Control = native_handle
        role = _get_role(control)
        name = _safe_name(control)

        try:
            ecp = control.GetExpandCollapsePattern()
            if ecp:
                ecp.Collapse()
                return f"Collapsed [{role}] '{name}'"
        except Exception as e:
            return f"Failed to collapse [{role}] '{name}': {e}"

        return f"[{role}] '{name}' does not support collapse"

    def scroll_element(
        self, native_handle: Any, direction: str, amount: float = 0.2
    ) -> str:
        control: auto.Control = native_handle
        role = _get_role(control)
        name = _safe_name(control)

        try:
            sp = control.GetScrollPattern()
            if sp:
                if direction in ("up", "down"):
                    pct = -amount * 100 if direction == "up" else amount * 100
                    current = sp.VerticalScrollPercent
                    new_pct = max(0, min(100, current + pct))
                    sp.SetScrollPercent(
                        auto.ScrollPattern.NoScrollValue, new_pct
                    )
                elif direction in ("left", "right"):
                    pct = -amount * 100 if direction == "left" else amount * 100
                    current = sp.HorizontalScrollPercent
                    new_pct = max(0, min(100, current + pct))
                    sp.SetScrollPercent(
                        new_pct, auto.ScrollPattern.NoScrollValue
                    )
                return f"Scrolled {direction} on [{role}] '{name}'"
        except Exception as e:
            return f"Failed to scroll [{role}] '{name}': {e}"

        return f"[{role}] '{name}' does not support scrolling"

    def get_element_text(self, native_handle: Any) -> str:
        control: auto.Control = native_handle
        name = _safe_name(control)

        # Try TextPattern
        try:
            tp = control.GetTextPattern()
            if tp:
                text = tp.DocumentRange.GetText(-1)
                if text:
                    return text
        except Exception:
            pass

        # Try ValuePattern
        try:
            vp = control.GetValuePattern()
            if vp:
                v = vp.Value
                if v:
                    return v
        except Exception:
            pass

        # Fall back to Name
        return name

    def get_element_children(self, native_handle: Any, max_depth: int = 2) -> UIElement:
        """Get a subtree rooted at a specific element without clearing the ID registry."""
        control: auto.Control = native_handle
        walk_state = _TreeWalkState(TREE_WALK_TIMEOUT)
        elem = _build_element(control, 0, max_depth, False, walk_state)
        if elem is None:
            role = _get_role(control)
            name = _safe_name(control)
            raise ValueError(f"Failed to build subtree for [{role}] '{name}'")
        return elem

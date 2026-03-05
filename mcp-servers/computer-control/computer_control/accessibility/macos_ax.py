"""macOS Accessibility API provider (AXUIElement via pyobjc). Stub — not yet implemented."""

from __future__ import annotations

from typing import Any, Optional

from .base import AccessibilityProvider
from .tree import UIElement


class MacOSAXProvider(AccessibilityProvider):
    """macOS Accessibility provider — requires pyobjc-framework-ApplicationServices."""

    def __init__(self):
        raise NotImplementedError(
            "macOS accessibility provider not yet implemented. "
            "Contributions welcome — see docs/COMPUTER_CONTROL_V2.md"
        )

    def get_ui_tree(self, window_title=None, max_depth=3, include_invisible=False) -> UIElement:
        raise NotImplementedError

    def find_elements(self, name=None, role=None, automation_id=None, value=None, window_title=None) -> list[UIElement]:
        raise NotImplementedError

    def get_focused_element(self) -> Optional[UIElement]:
        raise NotImplementedError

    def list_windows(self, title_filter=None) -> list[dict[str, Any]]:
        raise NotImplementedError

    def click_element(self, native_handle: Any) -> str:
        raise NotImplementedError

    def set_value(self, native_handle: Any, value: str) -> str:
        raise NotImplementedError

    def toggle_element(self, native_handle: Any) -> str:
        raise NotImplementedError

    def select_element(self, native_handle: Any) -> str:
        raise NotImplementedError

    def expand_element(self, native_handle: Any) -> str:
        raise NotImplementedError

    def collapse_element(self, native_handle: Any) -> str:
        raise NotImplementedError

    def scroll_element(self, native_handle: Any, direction: str, amount: float = 0.2) -> str:
        raise NotImplementedError

    def get_element_text(self, native_handle: Any) -> str:
        raise NotImplementedError

    def get_element_children(self, native_handle: Any, max_depth: int = 2) -> UIElement:
        raise NotImplementedError

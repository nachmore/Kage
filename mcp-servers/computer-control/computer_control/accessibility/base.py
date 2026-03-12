"""Abstract base class for platform-specific accessibility providers."""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any, Optional

from .tree import UIElement


class AccessibilityProvider(ABC):
    """Platform-specific accessibility tree provider."""

    @abstractmethod
    def get_ui_tree(
        self,
        window_title: Optional[str] = None,
        max_depth: int = 3,
        include_invisible: bool = False,
    ) -> UIElement:
        """Return the accessibility tree for a window.

        Args:
            window_title: Substring match on window title. Uses focused window if None.
            max_depth: Maximum depth to walk the tree.
            include_invisible: Whether to include offscreen/hidden elements.

        Returns:
            Root UIElement with children populated up to max_depth.
        """

    @abstractmethod
    def find_elements(
        self,
        name: Optional[str] = None,
        role: Optional[str] = None,
        automation_id: Optional[str] = None,
        value: Optional[str] = None,
        window_title: Optional[str] = None,
    ) -> list[UIElement]:
        """Search for elements matching criteria.

        Returns a flat list of matching elements (no children populated).
        """

    @abstractmethod
    def get_focused_element(self) -> Optional[UIElement]:
        """Return the currently focused element."""

    @abstractmethod
    def list_windows(
        self, title_filter: Optional[str] = None
    ) -> list[dict[str, Any]]:
        """Return all visible top-level windows (via accessibility API).

        Returns list of dicts with: title, bounds, process_name
        """

    # -- Action methods --

    @abstractmethod
    def click_element(self, native_handle: Any) -> str:
        """Invoke/press an element via accessibility API."""

    @abstractmethod
    def set_value(self, native_handle: Any, value: str) -> str:
        """Set text/value on an element."""

    @abstractmethod
    def toggle_element(self, native_handle: Any) -> str:
        """Toggle a checkbox/switch."""

    @abstractmethod
    def select_element(self, native_handle: Any) -> str:
        """Select an item in a list/combo/tab."""

    @abstractmethod
    def expand_element(self, native_handle: Any) -> str:
        """Expand a tree node, menu, or dropdown."""

    @abstractmethod
    def collapse_element(self, native_handle: Any) -> str:
        """Collapse a tree node, menu, or dropdown."""

    @abstractmethod
    def scroll_element(
        self, native_handle: Any, direction: str, amount: float = 0.2
    ) -> str:
        """Scroll within a scrollable container."""

    @abstractmethod
    def get_element_text(self, native_handle: Any) -> str:
        """Read text content from a text element."""

    @abstractmethod
    def get_element_children(
        self, native_handle: Any, max_depth: int = 2
    ) -> UIElement:
        """Get a subtree rooted at a specific element.

        Returns the element with its children populated up to max_depth.
        Useful for drilling into a specific part of the UI without
        re-fetching the entire window tree.
        """

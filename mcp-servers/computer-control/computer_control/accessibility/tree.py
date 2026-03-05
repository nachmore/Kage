"""UI element data structures, ID management, and tree serialization."""

from __future__ import annotations

import itertools
from dataclasses import dataclass, field
from typing import Any, Optional


# Monotonic counter for ephemeral element IDs
_id_counter = itertools.count(1)

# Global registry: id -> native handle (valid only for current snapshot)
_registry: dict[str, Any] = {}


def _next_id() -> str:
    return f"e{next(_id_counter)}"


class ElementID:
    """Manages ephemeral element IDs that map to native handles."""

    @staticmethod
    def register(native_handle: Any) -> str:
        """Register a native element handle and return an ephemeral ID."""
        eid = _next_id()
        _registry[eid] = native_handle
        return eid

    @staticmethod
    def resolve(eid: str) -> Any:
        """Resolve an ephemeral ID back to its native handle."""
        handle = _registry.get(eid)
        if handle is None:
            raise KeyError(
                f"Element '{eid}' not found. IDs are ephemeral — "
                f"call get_ui_tree() or find_elements() to get fresh IDs."
            )
        return handle

    @staticmethod
    def clear():
        """Clear all registered IDs. Call before building a new tree snapshot."""
        _registry.clear()

    @staticmethod
    def count() -> int:
        """Return the number of currently registered elements."""
        return len(_registry)


# Roles that are typically noise — no name, no actions, just structural clutter.
# These are skipped unless they have a name or actionable patterns.
NOISE_ROLES = frozenset({
    "separator", "thumb", "scrollbar", "image", "pane", "group", "header",
})


@dataclass
class UIElement:
    """A single UI element from the accessibility tree."""

    id: str
    role: str
    name: str = ""
    value: str = ""
    automation_id: str = ""
    states: list[str] = field(default_factory=list)
    actions: list[str] = field(default_factory=list)
    bounds: Optional[tuple[int, int, int, int]] = None  # (x, y, w, h)
    children: list[UIElement] = field(default_factory=list)

    def is_noise(self) -> bool:
        """Check if this element is structural noise with no useful info."""
        if self.role not in NOISE_ROLES:
            return False
        # Keep it if it has a meaningful name, value, or actions
        if self.name and self.name.strip():
            return False
        if self.value and self.value.strip():
            return False
        if self.actions:
            return False
        return True

    def count_elements(self) -> int:
        """Count total elements in this subtree."""
        total = 1
        for child in self.children:
            total += child.count_elements()
        return total

    def to_text(self, indent: int = 0, max_depth: int = 99) -> str:
        """Serialize to compact text tree format for LLM consumption.

        Noise elements (nameless separators, scrollbars, etc.) that have
        children are replaced by their children (flattened). Noise leaves
        are omitted entirely.
        """
        # If this is a noise leaf, skip it
        if self.is_noise() and not self.children:
            return ""

        # If this is a noise container, flatten — just render children
        if self.is_noise() and self.children:
            lines = []
            for child in self.children:
                text = child.to_text(indent, max_depth)
                if text:
                    lines.append(text)
            return "\n".join(lines)

        pad = "  " * indent
        parts = [f"{pad}[{self.role}]"]

        if self.name:
            # Truncate very long names
            n = self.name if len(self.name) <= 80 else self.name[:77] + "..."
            parts.append(f'"{n}"')

        parts.append(f"{{{self.id}}}")

        if self.value:
            v = self.value if len(self.value) <= 80 else self.value[:77] + "..."
            parts.append(f'value="{v}"')

        if self.states:
            parts.append(f"state=[{','.join(self.states)}]")

        if self.actions:
            parts.append(f"actions=[{','.join(self.actions)}]")

        if self.bounds:
            x, y, w, h = self.bounds
            parts.append(f"({w}x{h}@{x},{y})")

        lines = [" ".join(parts)]

        if indent < max_depth:
            for child in self.children:
                text = child.to_text(indent + 1, max_depth)
                if text:
                    lines.append(text)

        return "\n".join(lines)

"""Accessibility tree providers for cross-platform UI element discovery."""

from .tree import UIElement, ElementID
from .base import AccessibilityProvider
from .platform import get_provider

__all__ = ["UIElement", "ElementID", "AccessibilityProvider", "get_provider"]

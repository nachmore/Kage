"""Platform detection and provider factory."""

from __future__ import annotations

import platform
import logging

from .base import AccessibilityProvider

log = logging.getLogger("computer-control.platform")


def get_provider() -> AccessibilityProvider:
    """Return the appropriate accessibility provider for the current OS."""
    system = platform.system()

    if system == "Windows":
        from .windows_uia import WindowsUIAProvider
        return WindowsUIAProvider()
    elif system == "Darwin":
        from .macos_ax import MacOSAXProvider
        return MacOSAXProvider()
    elif system == "Linux":
        from .linux_atspi import LinuxATSPIProvider
        return LinuxATSPIProvider()
    else:
        raise RuntimeError(f"Unsupported platform: {system}")

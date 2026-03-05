"""Per-app steering: context-specific instructions for UI automation sub-agents.

App steering files live in the `app_steering/` directory next to this module.
Each file is a plain text file named after the app pattern it matches.
The orchestrator loads the relevant steering based on the app being automated.
"""

from __future__ import annotations

import logging
import os
from pathlib import Path
from typing import Optional

log = logging.getLogger("computer-control.steering")

# Directory containing app steering files
_STEERING_DIR = Path(__file__).parent / "app_steering"

# Cache: app_key -> steering text
_cache: dict[str, str] = {}


def _load_steering_files() -> dict[str, str]:
    """Load all steering files from the app_steering directory."""
    if _cache:
        return _cache

    if not _STEERING_DIR.exists():
        return _cache

    for f in _STEERING_DIR.glob("*.md"):
        key = f.stem.lower()  # e.g. "microsoft_office" from "microsoft_office.txt"
        try:
            _cache[key] = f.read_text(encoding="utf-8").strip()
            log.info("Loaded app steering: %s (%d chars)", key, len(_cache[key]))
        except Exception as e:
            log.warning("Failed to load steering %s: %s", f, e)

    return _cache


# Mapping of app name patterns to steering file keys
_APP_PATTERNS: dict[str, list[str]] = {
    "microsoft_office": [
        "word", "winword", "excel", "powerpnt", "powerpoint",
        "outlook", "onenote", "access", "publisher", "visio",
    ],
    "browser": [
        "chrome", "firefox", "edge", "msedge", "brave", "opera", "vivaldi",
    ],
    "notepad": ["notepad", "notepad++"],
    "calculator": ["calc", "calculator"],
    "paint": ["mspaint", "paint"],
    "terminal": ["cmd", "powershell", "windowsterminal", "terminal", "wt"],
}


def get_app_steering(app_name: str) -> Optional[str]:
    """Get app-specific steering for a given application name.

    Args:
        app_name: The application name (e.g. 'word', 'calc', 'chrome')

    Returns:
        Steering text if found, None otherwise.
    """
    steering_files = _load_steering_files()
    if not steering_files:
        return None

    app_lower = app_name.lower().strip()

    # Check each pattern group
    for steering_key, patterns in _APP_PATTERNS.items():
        for pattern in patterns:
            if pattern in app_lower or app_lower in pattern:
                if steering_key in steering_files:
                    log.info("Matched app '%s' to steering '%s'", app_name, steering_key)
                    return steering_files[steering_key]

    return None


def get_steering_for_task(task: str, details: str) -> Optional[str]:
    """Extract app name from a task description and return relevant steering.

    Scans the task and details text for known app names.

    Args:
        task: The task description (e.g. "Launch Microsoft Word")
        details: The task details

    Returns:
        Steering text if a matching app is found, None otherwise.
    """
    combined = f"{task} {details}".lower()

    # Check all known app patterns
    for steering_key, patterns in _APP_PATTERNS.items():
        for pattern in patterns:
            if pattern in combined:
                steering_files = _load_steering_files()
                if steering_key in steering_files:
                    return steering_files[steering_key]

    return None

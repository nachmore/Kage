#!/usr/bin/env python3
"""Enforce a production source-file size budget.

Files over the hard limit need a narrowly scoped exception here. Exceptions are
deliberate technical debt: keep the reason current and remove the entry when
the corresponding refactor lands.
"""

from __future__ import annotations

from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parent.parent
WARN_LINES = 500
HARD_LINES = 700
SOURCE_ROOTS = (ROOT / "src", ROOT / "ui" / "js")
EXTENSIONS = {".rs", ".js", ".jsx", ".ts", ".tsx"}

# Existing high-cohesion refactor targets. New files must not be added without
# an accompanying explanation and a concrete removal plan.
EXCEPTIONS = {
    "src/bin/computer_control_mcp.rs": "Split MCP schemas and command handlers.",
    "src/commands/window.rs": "Split by floating, chat, and auxiliary windows.",
    "src/os/macos/accessibility.rs": "Split native registry, traversal, and actions.",
    "src/updater.rs": "Split update checking, installation, and changelog fetch.",
    "src/setup.rs": "Split startup concerns by subsystem.",
    "src/main.rs": "Extract builder setup and run-event lifecycle.",
    "src/webview_recovery.rs": "Split detection, snapshots, and restart policy.",
    "src/agent_sessions/kiro_desktop.rs": "Split workspace and chat session parsers.",
    "src/os/windows/accessibility.rs": "Split native registry, traversal, and actions.",
    "src/activity_tracker.rs": "Split persistence, polling, and reporting.",
    "src/extensions.rs": "Split discovery, installation, and archive handling.",
    "src/acp_client/mod.rs": "Split client lifecycle from request operations.",
    "src/permission_audit.rs": "Split append/write and reverse-read operations.",
    "src/app_log.rs": "Split in-memory log state and file writer.",
    "src/commands/sessions/crud.rs": "Split watcher, scan, and session commands.",
    "ui/js/floating/app.js": "Split lifecycle, input, search, and message UI.",
    "ui/js/chat/app.js": "Split sessions, composer, and stream rendering.",
    "ui/js/extension-sandbox/runtime.js": "Split RPC, worker pool, and module loading.",
}


def line_count(path: Path) -> int:
    with path.open(encoding="utf-8") as handle:
        return sum(1 for _ in handle)


def main() -> int:
    warnings: list[tuple[str, int]] = []
    errors: list[tuple[str, int]] = []

    for source_root in SOURCE_ROOTS:
        for path in source_root.rglob("*"):
            if not path.is_file() or path.suffix not in EXTENSIONS:
                continue
            relative = path.relative_to(ROOT).as_posix()
            lines = line_count(path)
            if lines > HARD_LINES and relative not in EXCEPTIONS:
                errors.append((relative, lines))
            elif lines > WARN_LINES:
                warnings.append((relative, lines))

    for path, lines in sorted(warnings, key=lambda item: item[1], reverse=True):
        suffix = " (approved refactor target)" if path in EXCEPTIONS else ""
        print(f"module-size warning: {path}: {lines} lines{suffix}")

    if errors:
        for path, lines in errors:
            print(
                f"module-size error: {path}: {lines} lines exceeds the "
                f"{HARD_LINES}-line limit without an exception",
                file=sys.stderr,
            )
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())

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
SOURCE_ROOTS = (
    ROOT / "src",
    ROOT / "kage-core" / "src",
    ROOT / "computer_control_mcp" / "src",
    ROOT / "kage-calendar-helper" / "src",
    ROOT / "ui" / "js",
)
EXTENSIONS = {".rs", ".js", ".jsx", ".ts", ".tsx"}

# All production modules must remain within the hard limit.
EXCEPTIONS = {}


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

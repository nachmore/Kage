#!/usr/bin/env python3
"""Self-tests for ensure_vendor.py's "is ui/vendor/lib ready?" check.

A checkout whose ui/vendor/lib predates a newly vendored lib must re-run
`npm install` (and so setup.js): the sentinel list is what notices. These
tests check that every sentinel is a file setup.js actually produces, that
the agent sign-in's xterm bundle is one of them, and that a lib/ missing
only xterm triggers the install.

Run via: `python scripts/test_ensure_vendor.py`. Exits non-zero on failure.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path
from unittest import mock

SCRIPTS = Path(__file__).resolve().parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import ensure_vendor  # noqa: E402

try:
    sys.stdout.reconfigure(encoding="utf-8")  # type: ignore[attr-defined]
except AttributeError:
    pass


def test_every_sentinel_is_copied_by_setup_js() -> None:
    setup = (REPO / "ui-vendor" / "setup.js").read_text(encoding="utf-8")
    lib = Path("lib")
    for sentinel in ensure_vendor.vendor_sentinels(lib):
        rel = sentinel.relative_to(lib).as_posix()
        assert f"'{rel}'" in setup, f"setup.js never writes sentinel {rel}"


def test_xterm_is_a_sentinel() -> None:
    lib = Path("lib")
    rels = {p.relative_to(lib).as_posix() for p in ensure_vendor.vendor_sentinels(lib)}
    assert "xterm/xterm.js" in rels, rels


def _populate(root: Path, skip: str | None) -> None:
    lib = root / "ui" / "vendor" / "lib"
    for p in ensure_vendor.vendor_sentinels(lib):
        if p.relative_to(lib).as_posix() == skip:
            continue
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("x", encoding="utf-8")


def test_lib_without_xterm_reinstalls() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        _populate(root, skip="xterm/xterm.js")
        with mock.patch.object(ensure_vendor.subprocess, "run") as run:
            run.return_value = mock.Mock(returncode=0)
            ensure_vendor.ensure_vendor(root)
        assert run.call_count == 1, "a lib/ missing xterm must run npm install"
        assert run.call_args.args[0] == ["npm", "install"]


def test_complete_lib_is_a_no_op() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        _populate(root, skip=None)
        with mock.patch.object(ensure_vendor.subprocess, "run") as run:
            ensure_vendor.ensure_vendor(root)
        assert run.call_count == 0


def main() -> int:
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    failed = 0
    for test in tests:
        try:
            test()
            print(f"OK   {test.__name__}")
        except AssertionError as e:
            failed += 1
            print(f"FAIL {test.__name__}: {e}")
    print(f"{len(tests) - failed}/{len(tests)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

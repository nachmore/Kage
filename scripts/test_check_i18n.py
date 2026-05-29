#!/usr/bin/env python3
"""Self-tests for the i18n drift checker.

We exercise the failure modes the checker is meant to catch:
  - Source references a key not in the EN catalog (`missing in en`).
  - A non-EN catalog has a different key set than EN.
  - A non-EN catalog has the same key as EN but a different placeholder set.
  - Stale EN keys are reported as warnings (not errors).

Each test sets up a temporary mini-repo layout, runs `check_i18n.py` against
it, and asserts on the exit code and stdout/stderr.

Run via: `python scripts/test_check_i18n.py`. The script prints a one-line
summary and exits non-zero on the first failure.
"""

from __future__ import annotations
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "check_i18n.py"

# Force UTF-8 so the OK/FAIL markers don't crash on Windows cp1252.
try:
    sys.stdout.reconfigure(encoding="utf-8")  # type: ignore[attr-defined]
    sys.stderr.reconfigure(encoding="utf-8")  # type: ignore[attr-defined]
except (AttributeError, OSError):
    pass


def _scaffold_repo(tmp: Path, en: dict, others: dict[str, dict] | None = None,
                   rust_src: str = "", js_src: str = "") -> None:
    """Lay out a minimal repo at `tmp` matching what check_i18n.py expects."""
    (tmp / "locales" / "en").mkdir(parents=True)
    (tmp / "locales" / "en" / "messages.json").write_text(
        json.dumps(en, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    for code, cat in (others or {}).items():
        d = tmp / "locales" / code
        d.mkdir(parents=True)
        (d / "messages.json").write_text(
            json.dumps(cat, ensure_ascii=False, indent=2), encoding="utf-8"
        )
    (tmp / "src").mkdir(parents=True, exist_ok=True)
    (tmp / "src" / "main.rs").write_text(rust_src, encoding="utf-8")
    (tmp / "ui").mkdir(parents=True, exist_ok=True)
    (tmp / "ui" / "main.js").write_text(js_src, encoding="utf-8")


def _run_checker(tmp: Path) -> tuple[int, str]:
    """Run check_i18n.py with `tmp` as the project root by patching sys.argv
    indirectly: we exec the script as a subprocess and feed it cwd=tmp via env.

    The script computes ROOT from its own file location, so to actually trick
    it into using the temp dir we copy the script there.
    """
    # Copy the script and run from the temp dir so its path-relative ROOT lands
    # inside the scaffold.
    target = tmp / "scripts" / "check_i18n.py"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(SCRIPT.read_bytes())
    # Need a placeholder ../Kage-Extensions check directory to be skipped
    # cleanly (warning, not error). The script handles its absence already.
    proc = subprocess.run(
        [sys.executable, str(target)],
        cwd=tmp,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return proc.returncode, proc.stdout + proc.stderr


def expect(condition: bool, label: str) -> bool:
    if condition:
        print(f"  ✓ {label}")
        return True
    print(f"  ✗ {label}")
    return False


def test_clean_repo() -> bool:
    print("test_clean_repo")
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        en = {
            "_meta": {"language": "en", "name": "English", "rtl": False, "machine_translated": False},
            "greeting": {"message": "Hello {name}", "description": "Friendly hello"},
        }
        # The Rust and JS source must reference every EN key, otherwise we'd
        # get an "unused EN key" warning. The checker only fails on errors.
        _scaffold_repo(
            tmp, en,
            rust_src='fn x() { let _ = t!("greeting", "name" => "world"); }',
            js_src='import { t } from "./i18n.js"; console.log(t("greeting", { name: "world" }));',
        )
        rc, out = _run_checker(tmp)
        return all([
            expect(rc == 0, "exit code 0"),
            expect("drift check passed" in out.lower(), "passed message"),
        ])


def test_missing_en_key() -> bool:
    print("test_missing_en_key — source references a key not in EN")
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        _scaffold_repo(
            tmp,
            en={"_meta": {"language": "en"}, "greeting": {"message": "Hi", "description": ""}},
            rust_src='fn x() { let _ = t!("does.not.exist"); }',
        )
        rc, out = _run_checker(tmp)
        return all([
            expect(rc == 1, "exit code 1 (failure)"),
            expect("missing in en" in out.lower(), "reported missing key"),
            expect("does.not.exist" in out, "names the missing key"),
        ])


def test_non_en_drift_missing() -> bool:
    print("test_non_en_drift_missing — non-EN catalog missing keys")
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        en = {
            "_meta": {"language": "en"},
            "a": {"message": "A", "description": ""},
            "b": {"message": "B", "description": ""},
        }
        ja = {
            "_meta": {"language": "ja"},
            "a": {"message": "A日本語", "description": ""},
            # missing 'b' on purpose
        }
        _scaffold_repo(
            tmp, en, others={"ja": ja},
            rust_src='fn x() { let _ = t!("a"); let _ = t!("b"); }',
        )
        rc, out = _run_checker(tmp)
        return all([
            expect(rc == 1, "exit code 1"),
            expect("ja: missing key 'b'" in out, "reports the specific missing key"),
        ])


def test_non_en_extra_key() -> bool:
    print("test_non_en_extra_key — non-EN catalog has extra key")
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        en = {
            "_meta": {"language": "en"},
            "a": {"message": "A", "description": ""},
        }
        ja = {
            "_meta": {"language": "ja"},
            "a": {"message": "A", "description": ""},
            "extra": {"message": "Stray", "description": ""},
        }
        _scaffold_repo(tmp, en, others={"ja": ja}, rust_src='fn x() { let _ = t!("a"); }')
        rc, out = _run_checker(tmp)
        return all([
            expect(rc == 1, "exit code 1"),
            expect("extra key 'extra'" in out, "reports the extra key"),
        ])


def test_placeholder_drift() -> bool:
    print("test_placeholder_drift — translation drops a {placeholder}")
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        en = {
            "_meta": {"language": "en"},
            "greeting": {"message": "Hello {name}", "description": ""},
        }
        ja = {
            "_meta": {"language": "ja"},
            # Translator dropped the placeholder. Drift-check must catch this
            # because at runtime "Hello {name}" would render as a literal.
            "greeting": {"message": "こんにちは", "description": ""},
        }
        _scaffold_repo(
            tmp, en, others={"ja": ja},
            rust_src='fn x() { let _ = t!("greeting", "name" => "world"); }',
        )
        rc, out = _run_checker(tmp)
        return all([
            expect(rc == 1, "exit code 1"),
            expect("missing placeholders" in out, "reports placeholder drift"),
        ])


def test_unused_key_warns_not_errors() -> bool:
    print("test_unused_key_warns_not_errors")
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        en = {
            "_meta": {"language": "en"},
            "used": {"message": "U", "description": ""},
            "unused": {"message": "!", "description": ""},
        }
        _scaffold_repo(tmp, en, rust_src='fn x() { let _ = t!("used"); }')
        rc, out = _run_checker(tmp)
        return all([
            expect(rc == 0, "exit code 0 (warning, not error)"),
            expect("unused EN key" in out, "warning surfaces"),
        ])


TESTS = [
    test_clean_repo,
    test_missing_en_key,
    test_non_en_drift_missing,
    test_non_en_extra_key,
    test_placeholder_drift,
    test_unused_key_warns_not_errors,
]


def main() -> int:
    failed = 0
    for fn in TESTS:
        if not fn():
            failed += 1
        print()
    if failed:
        print(f"❌ {failed}/{len(TESTS)} tests failed")
        return 1
    print(f"✅ all {len(TESTS)} drift-check self-tests passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())

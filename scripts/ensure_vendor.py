"""Ensure ui/vendor/lib/ is populated with browser bundles.

Called from tauri.conf.json's beforeDevCommand (via dev_server.py) and
directly as its beforeBuildCommand. This is the single place that checks
whether vendor libs need installing.

The vendor libs (marked, mermaid, prismjs, etc.) are not checked into
git. Running `npm install` in ui-vendor/ downloads them and a
postinstall hook (setup.js) copies the browser-ready bundles into
ui/vendor/lib/. The npm machinery deliberately lives outside ui/ so
package.json and node_modules don't get brotli-embedded in the
shipped binary.

This script is a no-op when lib/ already exists.
"""

import os
import subprocess
import sys
from pathlib import Path

# Force UTF-8 output so Unicode characters render on Windows (cp1252 default).
if sys.stdout and hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
if sys.stderr and hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def ensure_vendor(repo_root: Path) -> None:
    vendor_tooling = repo_root / "ui-vendor"
    lib = repo_root / "ui" / "vendor" / "lib"
    # Sentinel check: every entry must exist for `lib/` to count as
    # "ready". Adding a new vendor lib here is the trigger for older
    # checkouts to re-run `npm install` + setup.js automatically.
    # Pick one canonical file per lib so this stays a quick existence
    # check — full coverage is enforced by setup.js itself.
    sentinels = [
        lib / "marked.min.js",
        lib / "mermaid.min.js",
        lib / "katex" / "katex.min.js",
    ]
    if all(p.is_file() for p in sentinels):
        return  # Already populated

    missing = [p.relative_to(repo_root) for p in sentinels if not p.is_file()]
    print(
        f"[ensure_vendor] ui/vendor/lib/ missing {missing} — running npm install...",
        flush=True,
    )
    result = subprocess.run(
        ["npm", "install"],
        cwd=vendor_tooling,
        # shell=True needed on Windows where npm is a .cmd script
        shell=(sys.platform == "win32"),
    )
    if result.returncode != 0:
        print(
            f"[ensure_vendor] ❌ npm install failed (exit {result.returncode}). "
            f"Install Node.js/npm and retry, or run manually: cd ui-vendor && npm install",
            flush=True,
        )
        sys.exit(result.returncode)
    print("[ensure_vendor] ✓ vendor libs ready.", flush=True)


if __name__ == "__main__":
    # When run directly, resolve repo root from script location
    repo_root = Path(__file__).resolve().parent.parent
    ensure_vendor(repo_root)

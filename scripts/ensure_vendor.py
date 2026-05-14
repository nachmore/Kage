"""Ensure ui/vendor/lib/ is populated with browser bundles.

Called from tauri.conf.json's beforeDevCommand and beforeBuildCommand
(via dev_server.py and build_mcp.py respectively). This is the single
place that checks whether vendor libs need installing.

The vendor libs (marked, mermaid, prismjs, etc.) are not checked into
git. Running `npm install` in ui/vendor/ downloads them and a
postinstall hook (setup.js) copies the browser-ready bundles into
ui/vendor/lib/.

This script is a no-op when lib/ already exists.
"""

import os
import subprocess
import sys
from pathlib import Path


def ensure_vendor(repo_root: Path) -> None:
    vendor_dir = repo_root / "ui" / "vendor"
    sentinel = vendor_dir / "lib" / "marked.min.js"

    if sentinel.is_file():
        return  # Already populated

    print("[ensure_vendor] ui/vendor/lib/ not found — running npm install...", flush=True)
    result = subprocess.run(["npm", "install"], cwd=vendor_dir)
    if result.returncode != 0:
        print(
            f"[ensure_vendor] ❌ npm install failed (exit {result.returncode}). "
            f"Install Node.js/npm and retry, or run manually: cd ui/vendor && npm install",
            flush=True,
        )
        sys.exit(result.returncode)
    print("[ensure_vendor] ✓ vendor libs ready.", flush=True)


if __name__ == "__main__":
    # When run directly, resolve repo root from script location
    repo_root = Path(__file__).resolve().parent.parent
    ensure_vendor(repo_root)

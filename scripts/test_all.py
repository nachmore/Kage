#!/usr/bin/env python3
"""Run all tests (Rust + JS). Cross-platform."""

import subprocess
import sys
import os
import platform

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IS_WINDOWS = platform.system() == "Windows"
failed = False


def run(label, cmd, cwd=ROOT):
    print(f"\n{'=' * 3} {label} {'=' * 3}")
    # On Windows, npm/npx need shell=True to resolve .cmd wrappers
    use_shell = IS_WINDOWS and any(c in ("npm", "npx") for c in (cmd if isinstance(cmd, list) else [cmd]))
    result = subprocess.run(cmd, cwd=cwd, shell=use_shell)
    if result.returncode != 0:
        global failed
        failed = True


# Rust tests — lib + integration tests. Parallelism is capped at jobs=2 in
# .cargo/config.toml to keep Tauri's compile graph from exhausting the
# Windows paging file. Binaries excluded via test=false.
run("Rust Tests", ["cargo", "test"])

# JS tests — install deps if needed
js_dir = os.path.join(ROOT, "ui", "tests")
if not os.path.isdir(os.path.join(js_dir, "node_modules")):
    run("JS Install", ["npm", "install", "--silent"], cwd=js_dir)

run("JS Tests", ["npx", "vitest", "run"], cwd=js_dir)

# Summary
print()
if failed:
    print("❌ Some tests failed")
    sys.exit(1)
else:
    print("✅ All tests passed")

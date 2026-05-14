"""Build the kage-computer-control-mcp sidecar, matching the main build's profile.

Invoked from `tauri.conf.json` → `beforeBuildCommand`. The sidecar is a
separate binary (see src/bin/computer_control_mcp.rs) that kage spawns
at runtime; shipping a release sidecar next to a debug kage.exe works
but wastes a minute or two of compile time and produces mismatched
symbols, so we mirror whatever profile the top-level Tauri build is in.

Tauri sets TAURI_ENV_DEBUG=true for `cargo tauri dev` and
`cargo tauri build --debug`, and to false (or leaves it unset) for a
regular release build. That's the signal we key off.

Tauri invokes `beforeBuildCommand` from the workspace's frontend dir
(ui/tests in our case, inherited from the dev server setup), so this
script explicitly cd's back to the repo root before calling cargo.

Exit code is forwarded so a compile failure here fails the Tauri build.
"""

import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from ensure_vendor import ensure_vendor  # noqa: E402


def main() -> int:
    debug = os.environ.get("TAURI_ENV_DEBUG", "").lower() == "true"
    cmd = ["cargo", "build", "--bin", "kage-computer-control-mcp"]
    if not debug:
        cmd.append("--release")
    # scripts/ sits at the repo root, so parent.parent lands there
    # regardless of Tauri's cwd for the hook.
    repo_root = Path(__file__).resolve().parent.parent

    # Ensure vendor JS libs are ready before building
    ensure_vendor(repo_root)

    print(
        f"[build_mcp] profile={'debug' if debug else 'release'} cwd={repo_root} -> {' '.join(cmd)}",
        flush=True,
    )
    return subprocess.call(cmd, cwd=repo_root)


if __name__ == "__main__":
    sys.exit(main())

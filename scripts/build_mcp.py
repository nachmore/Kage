"""Build the kage-computer-control-mcp sidecar, matching the main build's profile.

Invoked from `tauri.conf.json` → `beforeBuildCommand`. The sidecar is a
separate binary (see src/bin/computer_control_mcp.rs) that kage spawns
at runtime; shipping a release sidecar next to a debug kage.exe works
but wastes a minute or two of compile time and produces mismatched
symbols, so we mirror whatever profile the top-level Tauri build is in.

Tauri sets TAURI_ENV_DEBUG=true for `cargo tauri dev` and
`cargo tauri build --debug`, and to false (or leaves it unset) for a
regular release build. That's the signal we key off.

Tauri invokes `beforeBuildCommand` from a CWD that depends on the host
setup, so this script resolves the repo root from its own location and
passes it explicitly to subprocess.

Exit code is forwarded so a compile failure here fails the Tauri build.
"""

import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from ensure_vendor import ensure_vendor  # noqa: E402


def main() -> int:
    # Allow callers to skip the MCP build (e.g. run_bundled_dev.sh builds it
    # separately after Tauri finishes to avoid double-compiling deps).
    if os.environ.get("KAGE_SKIP_MCP_BUILD", "").strip() == "1":
        print("[build_mcp] Skipping MCP build (KAGE_SKIP_MCP_BUILD=1)", flush=True)
        # Still ensure vendor libs are ready
        repo_root = Path(__file__).resolve().parent.parent
        ensure_vendor(repo_root)
        return 0

    debug = os.environ.get("TAURI_ENV_DEBUG", "").lower() == "true"
    cmd = ["cargo", "build", "--bin", "kage-computer-control-mcp"]
    if not debug:
        cmd.append("--release")
    # scripts/ sits at the repo root, so parent.parent lands there
    # regardless of Tauri's cwd for the hook.
    repo_root = Path(__file__).resolve().parent.parent

    # Ensure vendor JS libs are ready before building
    ensure_vendor(repo_root)

    # Warn if signing key is missing (needed for bundled builds with updater)
    if not os.environ.get("TAURI_SIGNING_PRIVATE_KEY"):
        env_file = repo_root / ".env"
        if env_file.is_file():
            print(
                "[build_mcp] WARNING: TAURI_SIGNING_PRIVATE_KEY not set. "
                "The build will succeed but signing will fail.\n"
                "  macOS/Linux:  source .env && cargo tauri build --debug\n"
                "  Any platform: npx dotenv-cli -- cargo tauri build --debug\n"
                "  Generate keys: ./scripts/generate_signing_keys.sh",
                flush=True,
            )
        else:
            print(
                "[build_mcp] WARNING: TAURI_SIGNING_PRIVATE_KEY not set and no .env file found.\n"
                "  Run ./scripts/generate_signing_keys.sh to generate keys.",
                flush=True,
            )

    print(
        f"[build_mcp] profile={'debug' if debug else 'release'} cwd={repo_root} -> {' '.join(cmd)}",
        flush=True,
    )
    return subprocess.call(cmd, cwd=repo_root)


if __name__ == "__main__":
    sys.exit(main())

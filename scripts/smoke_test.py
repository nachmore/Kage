"""Launch the built Kage binary and verify it starts cleanly.

CI-runnable end-to-end smoke test. Catches the class of failure that
unit tests structurally can't: code that only runs inside a real Tauri
App/AppHandle (setup-phase wiring, state registration, plugin init).
A panic there may not even kill the process — Kage's panic hook writes
crash.log and spawned-task panics leave the app half-alive — so this
checks three things, not one:

  1. The process starts and stays up through setup.
  2. "=== Setup Complete ===" appears in app.jsonl within the timeout.
  3. NO crash-report bytes are written (the shipped changelog-cache
     panic would have failed exactly here while leaving the process
     running).

Sandboxing reality check: on macOS/Linux the `dirs` crate derives
everything from $HOME, so we point HOME at a temp dir and the run is
fully hermetic. On Windows `dirs` uses the Known Folder API
(SHGetKnownFolderPath), which IGNORES the APPDATA/LOCALAPPDATA env
vars — there is no env-level sandbox. There we run against the real
profile directories and detect *new* activity via before/after
snapshots (app.jsonl byte offset, crash.log size). CI runners are
clean VMs so this is equivalent to hermetic; on a dev machine the run
leaves normal app artifacts behind (same as launching Kage yourself).

Because Kage is single-instance, a second launch would signal the
running instance and exit — so the script refuses to run if Kage is
already up (dev-machine guard; never fires on CI).

Usage:
    python scripts/smoke_test.py [path-to-binary] [--timeout SECONDS]

Default binary: target/debug/kage(.exe) — the output of
`cargo tauri build --debug` (which embeds the frontend; a plain
`cargo build` binary would fail on the missing dev server and is not a
valid smoke target).

Exit code 0 = clean start; non-zero = failure (details on stdout).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

SETUP_MARKER = "=== Setup Complete ==="


def default_binary(repo_root: Path) -> Path:
    name = "kage.exe" if sys.platform == "win32" else "kage"
    return repo_root / "target" / "debug" / name


def kage_already_running() -> bool:
    """Single-instance guard: a second launch would just signal the
    running app and exit, which the test would misread as a crash."""
    try:
        if sys.platform == "win32":
            out = subprocess.run(
                ["tasklist", "/FI", "IMAGENAME eq kage.exe", "/FO", "CSV", "/NH"],
                capture_output=True,
                text=True,
                timeout=15,
            ).stdout
            return "kage.exe" in out
        out = subprocess.run(
            ["pgrep", "-x", "kage"], capture_output=True, text=True, timeout=15
        )
        return out.returncode == 0
    except OSError:
        return False


def data_local_dir(home_override: Path | None) -> Path:
    """Mirror `dirs::data_local_dir()` for the environment the app will
    actually see (env sandbox honored on macOS/Linux only)."""
    if sys.platform == "win32":
        return Path(os.environ["LOCALAPPDATA"])
    home = home_override or Path.home()
    if sys.platform == "darwin":
        return home / "Library" / "Application Support"
    return home / ".local" / "share"


def read_lines(path: Path) -> list[str]:
    try:
        return path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []


def file_size(path: Path) -> int:
    try:
        return path.stat().st_size
    except OSError:
        return 0


def tail_for_context(app_log: Path, start_offset: int, n: int = 25) -> str:
    try:
        with open(app_log, encoding="utf-8", errors="replace") as f:
            f.seek(start_offset)
            lines = f.read().splitlines()[-n:]
    except OSError:
        return "  <no app.jsonl>"
    out = []
    for line in lines:
        try:
            d = json.loads(line)
            out.append(
                f"  {d.get('ts','')} [{d.get('level','')}] {d.get('source','')}: {d.get('msg','')[:160]}"
            )
        except json.JSONDecodeError:
            out.append(f"  {line[:180]}")
    return "\n".join(out) or "  <no new log lines>"


def new_marker_since(app_log: Path, offset: int) -> bool:
    try:
        with open(app_log, encoding="utf-8", errors="replace") as f:
            f.seek(offset)
            return SETUP_MARKER in f.read()
    except OSError:
        return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", nargs="?", default=None)
    parser.add_argument("--timeout", type=int, default=90)
    parser.add_argument(
        "--linger",
        type=int,
        default=10,
        help="Seconds to keep watching AFTER setup completes, so background "
        "tasks spawned at the end of setup get a chance to panic while "
        "we're still looking.",
    )
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    binary = Path(args.binary) if args.binary else default_binary(repo_root)
    if not binary.is_file():
        print(f"[smoke] FAIL: binary not found at {binary}")
        print("[smoke] Build it first: cargo tauri build --debug")
        return 2

    if kage_already_running():
        print(
            "[smoke] FAIL: a Kage instance is already running. The app is "
            "single-instance, so this launch would just signal it and exit. "
            "Close Kage and re-run."
        )
        return 2

    env = os.environ.copy()
    env["RUST_BACKTRACE"] = "1"
    home_override: Path | None = None
    tmp_ctx = tempfile.TemporaryDirectory(prefix="kage-smoke-")
    if sys.platform != "win32":
        home_override = Path(tmp_ctx.name)
        env["HOME"] = str(home_override)

    logs_dir = data_local_dir(home_override) / "kage" / "logs"
    app_log = logs_dir / "app.jsonl"
    crash_log = logs_dir / "crash.log"

    # Snapshot pre-launch state — on Windows these files likely already
    # exist from normal use; only growth after this point counts.
    log_offset = file_size(app_log)
    crash_size_before = file_size(crash_log)

    print(f"[smoke] Launching {binary}")
    print(f"[smoke] Watching {logs_dir} (app.jsonl from byte {log_offset})")
    proc = subprocess.Popen(
        [str(binary)],
        env=env,
        cwd=repo_root,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    def crashed() -> bool:
        return file_size(crash_log) > crash_size_before

    failure = None
    setup_seen = False
    deadline = time.monotonic() + args.timeout
    try:
        # Phase 1: wait for setup to complete.
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                failure = f"process exited during startup (code {proc.returncode})"
                break
            if crashed():
                failure = "crash report written during startup"
                break
            if new_marker_since(app_log, log_offset):
                setup_seen = True
                print("[smoke] Setup completed OK")
                break
            time.sleep(0.5)
        else:
            failure = f"'{SETUP_MARKER}' not seen within {args.timeout}s"

        # Phase 2: linger — background tasks spawned at the tail of
        # setup (updater check, cache refreshes, registry scans) run
        # now. A panic here writes crash.log without necessarily
        # killing the process.
        if failure is None and setup_seen:
            linger_end = time.monotonic() + args.linger
            while time.monotonic() < linger_end:
                if proc.poll() is not None:
                    failure = f"process died after setup (code {proc.returncode})"
                    break
                if crashed():
                    failure = "crash report written after setup"
                    break
                time.sleep(0.5)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=15)
        tmp_ctx.cleanup()

    if failure:
        print(f"[smoke] FAIL: {failure}")
        if crashed():
            print("[smoke] --- crash.log (new content, first 60 lines) ---")
            try:
                with open(crash_log, encoding="utf-8", errors="replace") as f:
                    f.seek(crash_size_before)
                    for line in f.read().splitlines()[:60]:
                        print(f"  {line}")
            except OSError:
                pass
        print("[smoke] --- app.jsonl tail (this run) ---")
        print(tail_for_context(app_log, log_offset))
        return 1

    print("[smoke] PASS: clean start, setup complete, no crash report")
    return 0


if __name__ == "__main__":
    sys.exit(main())

"""
Headless slash-command probe (raw JSON-RPC, no acp library).

Spawns the real agent CLI over ACP via plain subprocess + newline-delimited
JSON-RPC, mirroring exactly what src/acp_client does. Captures the
`commands/available` notification (so we can see each command's meta —
inputType / optionsMethod / hint), then fires `commands/execute` for a few
selection-type commands and prints the FULL untruncated reply. Used to design
the shared selection picker against real agent output instead of guessing.

Usage:
    python scripts/probe_slash.py                       # default kiro-cli acp
    python scripts/probe_slash.py /agent /model /think  # probe specific commands
    python scripts/probe_slash.py -- <command> [args]   # custom agent command
"""

import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import time

VENDOR_PREFIXES = ["_kage.dev/", "_kiro.dev/"]


def _default_command() -> str:
    env = os.environ.get("ACP_AGENT_COMMAND") or os.environ.get("KIRO_CLI_PATH")
    if env:
        return env
    # Use the bare name, not shutil.which's resolved path: the toolbox shim
    # rejects an explicit "kiro-cli.EXE" ("doesn't appear to be associated
    # with any tool") but resolves the bare "kiro-cli" via its own lookup.
    return "kiro-cli"


def _print_help():
    print(
        """Headless ACP slash-command probe.

Spawns an ACP agent, captures its commands/available meta, and dumps the full
commands/execute reply for the given commands. No app build required.

Usage:
  python probe_slash.py                       Probe agent, model, context
  python probe_slash.py <cmd> [<cmd> ...]     Probe specific commands
  python probe_slash.py -- <command> [args]   Use a custom agent command
  python probe_slash.py --help                Show this help

Environment:
  ACP_AGENT_COMMAND   Override the default agent command (defaults to kiro-cli).
  KIRO_CLI_PATH       Legacy fallback for the same.

Note: pass bare command names ('context', not '/context') to avoid Git-Bash
rewriting a leading-slash arg into a fake path."""
    )


def _split_args(argv):
    """Returns (command, agent_args, commands_to_probe)."""
    command, agent_args = _default_command(), ["acp"]
    probe = []
    if "--help" in argv or "-h" in argv:
        _print_help()
        sys.exit(0)
    if "--" in argv:
        idx = argv.index("--")
        rest = argv[idx + 1:]
        argv = argv[:idx]
        if rest:
            command, agent_args = rest[0], rest[1:]
    for a in argv[1:]:
        # Take the last path segment to survive Git-Bash MSYS path conversion,
        # which rewrites a leading-slash arg like "/context" into a fake path
        # such as "C:/Program Files/Git/context". Bare names pass through
        # unchanged. Pass plain names ("context") to avoid the quirk entirely.
        probe.append(a.replace("\\", "/").rstrip("/").rsplit("/", 1)[-1])
    if not probe:
        probe = ["agent", "model", "context"]
    return command, agent_args, probe


def _dump(title, obj):
    print(f"\n===== {title} =====", flush=True)
    print(json.dumps(obj, indent=2, default=str), flush=True)


class RawAcp:
    """Minimal newline-delimited JSON-RPC driver over a subprocess."""

    def __init__(self, command, args):
        self.proc = subprocess.Popen(
            [command, *args],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self._next_id = 1
        self._responses = {}            # id -> result/error frame
        self._resp_lock = threading.Lock()
        self.notifications = []         # list of (method, params)
        self.vendor_prefix = None
        self._stop = False
        threading.Thread(target=self._read_loop, daemon=True).start()
        threading.Thread(target=self._stderr_loop, daemon=True).start()

    def _stderr_loop(self):
        for line in self.proc.stderr:
            txt = line.decode(errors="replace").rstrip()
            if txt:
                print(f"[agent-stderr] {txt}", flush=True)

    def _read_loop(self):
        for line in self.proc.stdout:
            txt = line.decode(errors="replace").strip()
            if not txt:
                continue
            try:
                msg = json.loads(txt)
            except json.JSONDecodeError:
                print(f"[non-json] {txt[:200]}", flush=True)
                continue
            method = msg.get("method")
            mid = msg.get("id")
            if method and mid is not None:
                # Server -> client REQUEST (e.g. session/request_permission,
                # fs/read_text_file). Auto-respond so the agent isn't blocked.
                self._handle_server_request(method, mid, msg.get("params") or {})
            elif method:
                # Notification
                self._observe_vendor(method)
                self.notifications.append((method, msg.get("params") or {}))
            elif mid is not None:
                with self._resp_lock:
                    self._responses[mid] = msg

    def _observe_vendor(self, method):
        for p in VENDOR_PREFIXES:
            if method.startswith(p):
                if self.vendor_prefix is None:
                    self.vendor_prefix = p
                break

    def _handle_server_request(self, method, mid, params):
        # Strip any vendor prefix for matching.
        suffix = method
        for p in VENDOR_PREFIXES:
            if method.startswith(p):
                suffix = method[len(p):]
                self._observe_vendor(method)
                break
        if suffix.endswith("request_permission"):
            opts = params.get("options") or []
            chosen = None
            for o in opts:
                oid = o.get("optionId") or o.get("option_id") or ""
                if "allow" in oid:
                    chosen = oid
                    break
            if chosen is None and opts:
                chosen = opts[0].get("optionId") or opts[0].get("option_id")
            self._send({"jsonrpc": "2.0", "id": mid,
                        "result": {"outcome": {"outcome": "selected", "optionId": chosen}}})
        elif suffix.endswith("read_text_file"):
            self._send({"jsonrpc": "2.0", "id": mid, "result": {"content": ""}})
        elif suffix.endswith("write_text_file"):
            self._send({"jsonrpc": "2.0", "id": mid, "result": {}})
        else:
            # Unknown server request — reply with empty result so it doesn't block.
            self._send({"jsonrpc": "2.0", "id": mid, "result": {}})

    def _send(self, obj):
        self.proc.stdin.write((json.dumps(obj) + "\n").encode())
        self.proc.stdin.flush()

    def request(self, method, params, timeout=30):
        mid = self._next_id
        self._next_id += 1
        self._send({"jsonrpc": "2.0", "id": mid, "method": method, "params": params})
        deadline = time.time() + timeout
        while time.time() < deadline:
            with self._resp_lock:
                if mid in self._responses:
                    return self._responses.pop(mid)
            if self.proc.poll() is not None:
                raise RuntimeError(f"agent exited (code {self.proc.returncode}) while awaiting '{method}'")
            time.sleep(0.02)
        raise TimeoutError(f"no response to '{method}' within {timeout}s")

    def vendor_method(self, suffix):
        return (self.vendor_prefix or "_kiro.dev/") + suffix

    def close(self):
        try:
            self.proc.kill()
        except Exception:
            pass


def main():
    command, agent_args, probe_cmds = _split_args(sys.argv)
    cwd = os.getcwd()
    print(f"Agent: {command} {' '.join(agent_args)}", flush=True)
    print(f"CWD:   {cwd}", flush=True)
    print(f"Probing: {', '.join('/' + c for c in probe_cmds)}", flush=True)

    acp = RawAcp(command, agent_args)
    try:
        init = acp.request("initialize", {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": True, "writeTextFile": True},
                "terminal": True,
            },
            "clientInfo": {"name": "kage", "title": "Kage", "version": "0.1.0"},
        })
        _dump("initialize reply", init.get("result", init))

        sess = acp.request("session/new", {"cwd": cwd, "mcpServers": []})
        sid = (sess.get("result") or {}).get("sessionId")
        print(f"\nSession: {sid}", flush=True)

        # Let commands/available land (usually pushed right after session/new).
        time.sleep(1.5)
        print(f"Vendor prefix observed: {acp.vendor_prefix or '(none yet)'}", flush=True)

        available = [p for (m, p) in acp.notifications
                     if m.endswith("commands/available")]
        if available:
            _dump("commands/available (raw params)", available[-1])
        else:
            seen = sorted({m for (m, _) in acp.notifications})
            print(f"\n[!] No commands/available captured. Notifications seen: {seen}", flush=True)

        for cmd in probe_cmds:
            try:
                reply = acp.request(acp.vendor_method("commands/execute"), {
                    "sessionId": sid,
                    "command": {"command": cmd, "args": {}},
                })
                if "error" in reply:
                    _dump(f"commands/execute '{cmd}' ERROR", reply["error"])
                else:
                    _dump(f"commands/execute '{cmd}' reply", reply.get("result"))
            except Exception as exc:
                _dump(f"commands/execute '{cmd}' EXCEPTION", str(exc))
    finally:
        acp.close()


if __name__ == "__main__":
    main()

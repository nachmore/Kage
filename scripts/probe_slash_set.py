"""
Second probe: how does a selection-type command actually SET a value, and what
does the advertised optionsMethod return?

Reuses RawAcp from probe_slash.py. Fires, in order:
  1. optionsMethod for /agent and /model (the canonical option source)
  2. several candidate arg-shapes for the SET operation, reading `current`
     back from a follow-up list call to see which one actually switched.

Harmless: runs in a throwaway session that's killed on exit.
"""

import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from probe_slash import RawAcp, _default_command, _dump  # noqa: E402


def list_current(acp, sid, cmd):
    """Return the `current` value reported by a no-arg list call."""
    reply = acp.request(acp.vendor_method("commands/execute"), {
        "sessionId": sid,
        "command": {"command": cmd, "args": {}},
    })
    return (reply.get("result") or {}).get("data", {}).get("current")


def try_set(acp, sid, cmd, args, label):
    print(f"\n----- SET {cmd} via {label}: args={json.dumps(args)} -----", flush=True)
    try:
        reply = acp.request(acp.vendor_method("commands/execute"), {
            "sessionId": sid,
            "command": {"command": cmd, "args": args},
        })
        res = reply.get("result")
        if "error" in reply:
            print(f"  ERROR: {json.dumps(reply['error'])}", flush=True)
            return
        # Print a compact view: success flag + message head + current.
        msg = (res or {}).get("message", "")
        cur = (res or {}).get("data", {}).get("current")
        print(f"  success={res.get('success') if isinstance(res, dict) else '?'} "
              f"current_in_reply={cur!r}", flush=True)
        print(f"  message head: {msg.splitlines()[0] if msg else '(empty)'}", flush=True)
    except Exception as exc:
        print(f"  EXCEPTION: {exc}", flush=True)


def main():
    command, agent_args = _default_command(), ["acp"]
    cwd = os.getcwd()
    acp = RawAcp(command, agent_args)
    try:
        acp.request("initialize", {
            "protocolVersion": 1,
            "clientCapabilities": {"fs": {"readTextFile": True, "writeTextFile": True}, "terminal": True},
            "clientInfo": {"name": "kage", "title": "Kage", "version": "0.1.0"},
        })
        sess = acp.request("session/new", {"cwd": cwd, "mcpServers": []})
        sid = (sess.get("result") or {}).get("sessionId")
        time.sleep(1.2)
        print(f"Session: {sid}  vendor={acp.vendor_prefix}", flush=True)

        # 1. Canonical optionsMethod replies.
        for cmd in ("agent", "model"):
            try:
                reply = acp.request(f"{acp.vendor_prefix}commands/{cmd}/options",
                                    {"sessionId": sid})
                _dump(f"commands/{cmd}/options reply", reply.get("result", reply))
            except Exception as exc:
                _dump(f"commands/{cmd}/options EXCEPTION", str(exc))

        # 2. Candidate SET shapes for /agent.
        print(f"\n### /agent current before: {list_current(acp, sid, 'agent')!r}", flush=True)
        try_set(acp, sid, "agent", {"agentName": "kiro_planner"}, "agentName")
        print(f"### /agent current after agentName: {list_current(acp, sid, 'agent')!r}", flush=True)
        try_set(acp, sid, "agent", {"input": "kiro_guide"}, "input=name")
        print(f"### /agent current after input=name: {list_current(acp, sid, 'agent')!r}", flush=True)
        try_set(acp, sid, "agent", {"input": "swap kiro_planner"}, "input=swap name")
        print(f"### /agent current after input=swap: {list_current(acp, sid, 'agent')!r}", flush=True)

        # 3. Candidate SET shapes for /model.
        print(f"\n### /model current before: {list_current(acp, sid, 'model')!r}", flush=True)
        try_set(acp, sid, "model", {"modelName": "claude-sonnet-4.5"}, "modelName")
        print(f"### /model current after modelName: {list_current(acp, sid, 'model')!r}", flush=True)
        try_set(acp, sid, "model", {"input": "claude-haiku-4.5"}, "input=id")
        print(f"### /model current after input=id: {list_current(acp, sid, 'model')!r}", flush=True)
    finally:
        acp.close()


if __name__ == "__main__":
    main()

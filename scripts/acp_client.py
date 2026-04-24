"""
ACP interactive client with dual-window UI.

The main window shows raw JSON-RPC trace. A second console window opens
for the chat interface. They communicate over a local TCP socket.

Usage:
    python acp-client.py                          # uses default command
    python acp-client.py -- <command> [args...]    # custom agent command
"""

import asyncio
import json
import os
import subprocess
import sys
from datetime import datetime
from typing import Any

from acp import spawn_agent_process, text_block, PROTOCOL_VERSION
from acp.connection import StreamDirection, StreamEvent
from acp.interfaces import Client

def _default_cli_command() -> str:
    """Resolve the default kage-cli command: env override → PATH lookup → bare name."""
    env = os.environ.get("KAGE_CLI_PATH")
    if env:
        return env
    import shutil
    found = shutil.which("kage-cli") or shutil.which("kage-cli.exe")
    return found or "kage-cli"


DEFAULT_COMMAND = _default_cli_command()
DEFAULT_ARGS = ["acp"]

# ANSI colours
DIM = "\033[2m"
CYAN = "\033[36m"
GREEN = "\033[32m"
YELLOW = "\033[33m"
RED = "\033[31m"
RESET = "\033[0m"
BOLD = "\033[1m"


def trace(event: StreamEvent) -> None:
    """Print every JSON-RPC frame to the trace window."""
    ts = datetime.now().strftime("%H:%M:%S.%f")[:-3]
    if event.direction == StreamDirection.OUTGOING:
        arrow = f"{CYAN}>>> CLIENT → AGENT  {DIM}{ts}{RESET}"
    else:
        arrow = f"{YELLOW}<<< AGENT → CLIENT  {DIM}{ts}{RESET}"
    method = event.message.get("method", "")
    msg_id = event.message.get("id", "")
    header = method or (f"response id={msg_id}" if msg_id != "" else "")
    pretty = json.dumps(event.message, indent=2)
    print(f"\n{arrow} {DIM}{header}{RESET}\n{DIM}{pretty}{RESET}", flush=True)


class JsonLineProtocol:
    """Simple newline-delimited JSON over TCP for chat ↔ trace IPC."""

    @staticmethod
    def encode(obj: dict) -> bytes:
        return (json.dumps(obj) + "\n").encode()

    @staticmethod
    def decode(line: bytes) -> dict:
        return json.loads(line.decode().strip())


class InteractiveClient(Client):
    """ACP Client that forwards agent output to the chat window over TCP."""

    def __init__(self):
        self._chat_writer: asyncio.StreamWriter | None = None

    def set_chat_writer(self, writer: asyncio.StreamWriter):
        self._chat_writer = writer

    async def _send_to_chat(self, msg: dict):
        if self._chat_writer and not self._chat_writer.is_closing():
            self._chat_writer.write(JsonLineProtocol.encode(msg))
            await self._chat_writer.drain()

    async def request_permission(self, options, session_id, tool_call, **kw):
        # Auto-approve: pick the first "allow" option, fall back to first option
        option_id = options[0].option_id
        for opt in options:
            if "allow" in (opt.option_id or ""):
                option_id = opt.option_id
                break
        return {"outcome": {"outcome": "selected", "optionId": option_id}}


    async def session_update(self, session_id, update, **kw):
        kind = getattr(update, "session_update", None)
        content = getattr(update, "content", None)
        if kind == "agent_message_chunk" and content:
            text = getattr(content, "text", None)
            if text:
                await self._send_to_chat({"type": "text", "text": text})
        elif kind == "tool_call_start":
            name = getattr(update, "name", "?")
            await self._send_to_chat({"type": "tool", "name": name})
        elif kind == "tool_call_progress":
            status = getattr(update, "status", None)
            if status:
                await self._send_to_chat({"type": "status", "status": status})

    async def write_text_file(self, content, path, session_id, **kw):
        await self._send_to_chat({"type": "info", "text": f"[write: {path}]"})
        return None

    async def read_text_file(self, path, session_id, **kw):
        try:
            with open(path, encoding="utf-8") as f:
                return {"content": f.read()}
        except Exception as exc:
            return {"content": "", "error": str(exc)}

    async def create_terminal(self, command, session_id, **kw):
        return {"terminalId": "unsupported"}

    async def terminal_output(self, session_id, terminal_id, **kw):
        return {"output": "", "exitCode": None}

    async def release_terminal(self, session_id, terminal_id, **kw):
        return None

    async def wait_for_terminal_exit(self, session_id, terminal_id, **kw):
        return {"exitCode": 1}

    async def kill_terminal(self, session_id, terminal_id, **kw):
        return None

    async def ext_method(self, method, params):
        return {}

    async def ext_notification(self, method, params):
        pass

    def on_connect(self, conn):
        pass


def parse_command(argv: list[str]) -> tuple[str, list[str]]:
    if "--" in argv:
        idx = argv.index("--")
        parts = argv[idx + 1:]
        if not parts:
            print("Error: no command after '--'", file=sys.stderr)
            sys.exit(1)
        return parts[0], parts[1:]
    return DEFAULT_COMMAND, list(DEFAULT_ARGS)


# Path to the chat UI script (lives next to this file)
CHAT_UI_SCRIPT_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "acp_chat_ui.py")


# ── Main process (trace window + ACP engine) ───────────────────────────

async def main() -> None:
    command, args = parse_command(sys.argv)
    cwd = os.getcwd()

    # Start a TCP server for the chat window to connect to
    chat_connected: asyncio.Future[tuple[asyncio.StreamReader, asyncio.StreamWriter]] = (
        asyncio.get_running_loop().create_future()
    )

    async def on_chat_connect(reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        if not chat_connected.done():
            chat_connected.set_result((reader, writer))

    server = await asyncio.start_server(on_chat_connect, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]

    print(f"{BOLD}{CYAN}╔══════════════════════════════════════╗{RESET}")
    print(f"{BOLD}{CYAN}║   ACP JSON-RPC Trace Window         ║{RESET}")
    print(f"{BOLD}{CYAN}╚══════════════════════════════════════╝{RESET}")
    print(f"{DIM}Agent:  {command} {' '.join(args)}{RESET}")
    print(f"{DIM}CWD:    {cwd}{RESET}")
    print(f"{DIM}Chat window opening on port {port}...{RESET}\n")

    # Spawn chat window using the standalone acp_chat_ui.py
    chat_proc = subprocess.Popen(
        ["cmd", "/c", "start", "ACP Chat", "cmd", "/k",
         sys.executable, CHAT_UI_SCRIPT_PATH, str(port)],
        creationflags=subprocess.CREATE_NEW_CONSOLE if sys.platform == "win32" else 0,
    )

    # Wait for chat window to connect
    try:
        chat_reader, chat_writer = await asyncio.wait_for(chat_connected, timeout=30)
    except asyncio.TimeoutError:
        print(f"{RED}Chat window didn't connect within 30s. Exiting.{RESET}")
        return

    print(f"{GREEN}Chat window connected.{RESET}\n")

    client = InteractiveClient()
    client.set_chat_writer(chat_writer)

    try:
        async with spawn_agent_process(client, command, *args) as (conn, proc):
            conn._conn.add_observer(trace)

            await conn.initialize(protocol_version=PROTOCOL_VERSION)
            session = await conn.new_session(cwd=cwd, mcp_servers=[])
            sid = session.session_id

            print(f"{GREEN}Session: {sid}{RESET}\n")

            # Tell chat window we're ready
            chat_writer.write(JsonLineProtocol.encode({"type": "ready", "session": sid}))
            await chat_writer.drain()

            # Read prompts from chat window, forward to agent
            while True:
                line = await chat_reader.readline()
                if not line:
                    break
                msg = JsonLineProtocol.decode(line)
                if msg.get("type") == "raw_jsonrpc":
                    payload = msg["payload"]
                    # Inject/override sessionId in params
                    if "params" in payload and isinstance(payload["params"], dict):
                        payload["params"]["sessionId"] = sid
                    elif "method" in payload:
                        payload.setdefault("params", {})["sessionId"] = sid
                    raw_conn = conn._conn
                    has_method = "method" in payload
                    has_id = "id" in payload
                    if has_method and has_id:
                        # JSON-RPC request — send and await response
                        try:
                            result = await raw_conn.send_request(
                                payload["method"], payload.get("params")
                            )
                            await client._send_to_chat(
                                {"type": "info", "text": f"[raw response: {json.dumps(result, indent=2)}]"}
                            )
                        except Exception as exc:
                            await client._send_to_chat(
                                {"type": "info", "text": f"[raw error: {exc}]"}
                            )
                    elif has_method:
                        # JSON-RPC notification — fire and forget
                        await raw_conn.send_notification(
                            payload["method"], payload.get("params")
                        )
                        await client._send_to_chat(
                            {"type": "info", "text": "[notification sent]"}
                        )
                    else:
                        # Arbitrary payload — send as-is
                        await raw_conn._sender.send(payload)
                        raw_conn._notify_observers(StreamDirection.OUTGOING, payload)
                        await client._send_to_chat(
                            {"type": "info", "text": "[raw payload sent]"}
                        )
                elif msg.get("type") == "subagent":
                    # Build the invoke_subagents command payload
                    subagent_entry: dict[str, Any] = {
                        "query": msg["query"],
                    }
                    if msg.get("agent_name"):
                        subagent_entry["agent_name"] = msg["agent_name"]
                    if msg.get("relevant_context"):
                        subagent_entry["relevant_context"] = msg["relevant_context"]

                    payload = {
                        "command": "invoke_subagents",
                        "content": {
                            "subagents": [subagent_entry],
                        },
                    }
                    try:
                        result = await conn.prompt(
                            session_id=sid,
                            prompt=[text_block(json.dumps(payload))],
                        )
                        await client._send_to_chat({"type": "done"})
                    except Exception as exc:
                        await client._send_to_chat(
                            {"type": "info", "text": f"[subagent error: {exc}]"}
                        )
                        await client._send_to_chat({"type": "done"})

                elif msg.get("type") == "prompt":
                    await conn.prompt(
                        session_id=sid,
                        prompt=[text_block(msg["text"])],
                    )
                    # Signal end of response
                    await client._send_to_chat({"type": "done"})

    except (KeyboardInterrupt, EOFError):
        print(f"\n{DIM}Shutting down.{RESET}")
    except FileNotFoundError:
        print(f"\n{RED}Could not launch agent: {command} {' '.join(args)}{RESET}")
        print(f"{RED}Make sure the command exists and is on PATH.{RESET}")
    except Exception as exc:
        print(f"\n{RED}Error: {exc}{RESET}")
    finally:
        chat_writer.close()
        server.close()


if __name__ == "__main__":
    asyncio.run(main())

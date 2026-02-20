"""
ACP Chat UI window.

Connects back to the trace process over a local TCP socket.
Launched automatically by acp_client.py — but can be run standalone for debugging:

    python acp_chat_ui.py <port>
"""

import asyncio
import itertools
import json
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 0

# ANSI colours
GREEN = "\033[32m"
DIM = "\033[2m"
YELLOW = "\033[33m"
RESET = "\033[0m"
BOLD = "\033[1m"
CLEAR_LINE = "\033[2K\r"


async def main():
    if not PORT:
        print("Usage: python acp_chat_ui.py <port>", file=sys.stderr)
        sys.exit(1)

    reader, writer = await asyncio.open_connection("127.0.0.1", PORT)
    print(f"{BOLD}{GREEN}ACP Chat{RESET}")
    print(f"{DIM}Connected to trace window on port {PORT}{RESET}")
    print(f"{DIM}Type messages below. Prefix with : to send raw JSON-RPC.{RESET}")
    print(f"{DIM}Ctrl+C to quit.{RESET}\n")

    loop = asyncio.get_running_loop()
    done_event = asyncio.Event()
    got_first_text = asyncio.Event()

    async def read_responses():
        while True:
            line = await reader.readline()
            if not line:
                print(f"\n{DIM}Connection closed.{RESET}")
                return
            msg = json.loads(line.decode().strip())
            t = msg.get("type")
            if t == "text":
                if not got_first_text.is_set():
                    print(f"{CLEAR_LINE}{GREEN}Agent>{RESET} ", end="", flush=True)
                    got_first_text.set()
                print(msg["text"], end="", flush=True)
            elif t == "tool":
                if not got_first_text.is_set():
                    print(f"{CLEAR_LINE}", end="", flush=True)
                    got_first_text.set()
                print(f"\n{DIM}[tool: {msg['name']}]{RESET}", flush=True)
            elif t == "status":
                print(f"{DIM}[{msg['status']}]{RESET}", flush=True)
            elif t == "info":
                print(f"{YELLOW}{msg['text']}{RESET}", flush=True)
            elif t == "ready":
                pass
            elif t == "done":
                print("\n")  # blank line after agent response
                done_event.set()

    async def thinking_animation():
        """Show a dots animation until the first text arrives."""
        frames = itertools.cycle([".  ", ".. ", "...", "   "])
        try:
            while not got_first_text.is_set():
                print(
                    f"{CLEAR_LINE}{DIM}{next(frames)} thinking...{RESET}",
                    end="",
                    flush=True,
                )
                await asyncio.sleep(0.1)
        except asyncio.CancelledError:
            pass

    resp_task = asyncio.create_task(read_responses())

    try:
        while True:
            line = await loop.run_in_executor(
                None, lambda: input(f"{BOLD}You>{RESET} ")
            )
            if not line.strip():
                continue

            # Raw JSON-RPC mode: lines starting with ':'
            if line.startswith(":"):
                raw = line[1:]
                try:
                    payload = json.loads(raw)
                except json.JSONDecodeError as e:
                    print(f"{YELLOW}Invalid JSON: {e}{RESET}")
                    continue
                if not isinstance(payload, dict):
                    print(f"{YELLOW}JSON must be an object{RESET}")
                    continue
                writer.write(
                    (json.dumps({"type": "raw_jsonrpc", "payload": payload}) + "\n").encode()
                )
                await writer.drain()
                print(f"{DIM}Sent raw JSON-RPC{RESET}")
                continue

            # Reset events for this turn
            done_event.clear()
            got_first_text.clear()

            # Send prompt
            writer.write(
                (json.dumps({"type": "prompt", "text": line}) + "\n").encode()
            )
            await writer.drain()

            # Show thinking animation until first text chunk arrives
            spinner = asyncio.create_task(thinking_animation())

            # Wait for the full response to finish
            await done_event.wait()
            spinner.cancel()

    except (KeyboardInterrupt, EOFError):
        print(f"\n{DIM}Bye.{RESET}")
    finally:
        resp_task.cancel()
        writer.close()


if __name__ == "__main__":
    asyncio.run(main())

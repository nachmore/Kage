"""Dev server for Tauri dev mode. Serves ui/ on port 1420."""
import os
import sys
import socket
import signal
import subprocess
import platform
import http.server

PORT = 1420

# If just diagnosing, exit early
if "--check" in sys.argv:
    print(f"CWD: {os.getcwd()}")
    print(f"ui exists: {os.path.isdir('ui')}")
    print(f"ui/index.html exists: {os.path.isfile('ui/index.html')}")
    sys.exit(0)

# Resolve repo root (scripts/ is one level deep) and the ui/ directory
repo_root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
ui_dir = os.path.join(repo_root, "ui")

# Ensure vendor JS libs are installed before serving
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ensure_vendor import ensure_vendor  # noqa: E402
from pathlib import Path
ensure_vendor(Path(repo_root))


def port_in_use(port):
    """Quick check if a port is already bound. Returns False on socket errors
    so a transient network stack hiccup doesn't crash startup."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(1.0)
            return s.connect_ex(("127.0.0.1", port)) == 0
    except OSError as e:
        print(f"Warning: port check failed ({e}); assuming free")
        return False


def kill_port_owner(port):
    """Kill whatever process is listening on the given port."""
    system = platform.system()
    try:
        if system == "Windows":
            # Use PowerShell — much faster than netstat
            result = subprocess.run(
                ["powershell", "-NoProfile", "-Command",
                 f"(Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue).OwningProcess"],
                capture_output=True, text=True, timeout=5
            )
            for pid_str in result.stdout.strip().splitlines():
                pid_str = pid_str.strip()
                if pid_str.isdigit():
                    pid = int(pid_str)
                    if pid != os.getpid() and pid != 0:
                        print(f"Killing existing server (PID {pid}) on port {port}")
                        subprocess.run(["taskkill", "/F", "/PID", str(pid)],
                                       capture_output=True, timeout=5)
        else:
            result = subprocess.run(
                ["lsof", "-ti", f"tcp:{port}"],
                capture_output=True, text=True, timeout=5
            )
            for pid_str in result.stdout.strip().splitlines():
                pid = int(pid_str)
                if pid != os.getpid():
                    print(f"Killing existing server (PID {pid}) on port {port}")
                    os.kill(pid, signal.SIGKILL)
    except Exception as e:
        print(f"Warning: could not kill port owner: {e}")


# Only scan for existing servers if the port is actually in use
if port_in_use(PORT):
    print(f"Port {PORT} is in use, killing existing server...")
    kill_port_owner(PORT)
    # Brief wait for the OS to release the socket
    import time
    time.sleep(0.3)


def build_mcp_binary():
    """Rebuild the standalone kage-computer-control-mcp binary.

    `cargo tauri dev` only rebuilds the main `kage` binary — edits to
    `computer_control_mcp/src/main.rs` (or to any module it pulls in,
    notably `src/os/accessibility.rs`) silently produce a stale MCP
    binary unless the developer remembers to run this command by hand.
    Running it here on every dev start makes the dev loop honest:
    the binary the agent spawns is always up-to-date with the source.

    Cargo's incremental cache makes this a no-op when nothing changed
    (sub-second on a warm tree); when it isn't, the dev server start
    is delayed by the rebuild, which is the correct trade-off — we'd
    rather wait than ship a stale binary into a debugging session.

    A failure here is fatal: continuing would launch the app against
    an old (or missing) binary and produce confusing runtime errors.
    """
    print("Building kage-computer-control-mcp...")
    sys.stdout.flush()
    result = subprocess.run(
        ["cargo", "build", "--package", "kage-computer-control-mcp"],
        cwd=repo_root,
        # Pipe through so build progress and any compile errors show up
        # in the same terminal as the dev server.
    )
    if result.returncode != 0:
        print(f"\n❌ kage-computer-control-mcp build failed (exit {result.returncode}); aborting dev start")
        sys.exit(result.returncode)


if "--no-mcp-build" not in sys.argv:
    build_mcp_binary()


class NoCacheHandler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=ui_dir, **kwargs)

    def handle(self):
        """Suppress broken pipe / connection aborted errors on Windows."""
        try:
            super().handle()
        except (BrokenPipeError, ConnectionAbortedError, ConnectionResetError):
            pass

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        # The extension sandbox iframe has `sandbox="allow-scripts"` with no
        # `allow-same-origin`, so it gets a *null* origin. In tauri prod
        # builds, assets are served via a custom protocol that doesn't
        # enforce CORS the same way. In dev mode we have a real HTTP server,
        # and null-origin iframes loading `<script type="module">` from it
        # hit the CORS wall — so we advertise `*` on every dev response.
        # This is dev-only; the file-serving origin here is localhost.
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()

    def do_GET(self):
        """Handle GET requests — serve files, mock store, or fall through to ui/."""
        if self.path == "/catalog.json" or self.path.startswith("/detail/") or self.path.startswith("/packages/"):
            return self._serve_local_catalog()
        return super().do_GET()

    def _serve_local_catalog(self):
        """Serve the Kage-Extensions repo's `dist/` output if it exists next door.

        In dev mode, the Rust client treats `http://localhost:1420` as the
        store base. We don't ship a mock catalog any more — the production
        flow is the static catalog at `https://nachmore.github.io/Kage-Extensions/`.
        For local-only development of an extension you can run
        `npm run build` inside `../Kage-Extensions` and the resulting
        `dist/` directory is served through here at the same paths the
        client would otherwise hit on Pages.
        """
        ext_dist = os.path.normpath(os.path.join(repo_root, "..", "Kage-Extensions", "dist"))
        if not os.path.isdir(ext_dist):
            self.send_response(404)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
            self.wfile.write(
                b"No local catalog found. Either:\n"
                b"  1) Clone Kage-Extensions next to Kage and run `npm run build`, or\n"
                b"  2) Use the production catalog at https://nachmore.github.io/Kage-Extensions/\n"
                b"     (Settings -> Store -> Custom store URL).\n"
            )
            return

        # Strip the leading slash and resolve safely under ext_dist.
        safe_rel = self.path.lstrip("/")
        target = os.path.normpath(os.path.join(ext_dist, safe_rel))
        if not target.startswith(ext_dist + os.sep) and target != ext_dist:
            self.send_response(403)
            self.end_headers()
            return
        if not os.path.isfile(target):
            self.send_response(404)
            self.end_headers()
            return

        # Pick a content type based on extension; only json + zip are expected here.
        if target.endswith(".json"):
            ctype = "application/json"
        elif target.endswith(".zip"):
            ctype = "application/zip"
        else:
            ctype = "application/octet-stream"

        with open(target, "rb") as f:
            data = f.read()
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format, *args):
        # Only log non-200 responses to reduce noise
        if len(args) >= 2 and str(args[1]) == "200":
            return
        super().log_message(format, *args)


class RobustHTTPServer(http.server.ThreadingHTTPServer):
    """Threaded HTTPServer that doesn't die on client connection errors."""
    allow_reuse_address = True
    daemon_threads = True

    def handle_error(self, request, client_address):
        """Suppress connection errors — these are normal when clients disconnect."""
        exc_type = sys.exc_info()[0]
        if exc_type in (BrokenPipeError, ConnectionAbortedError, ConnectionResetError, OSError):
            return
        super().handle_error(request, client_address)


server = RobustHTTPServer(("", PORT), NoCacheHandler)
print(f"Dev server running on http://localhost:{PORT}")
sys.stdout.flush()
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()

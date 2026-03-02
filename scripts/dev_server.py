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


def port_in_use(port):
    """Quick check if a port is already bound."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        return s.connect_ex(("127.0.0.1", port)) == 0


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

# Resolve ui/ directory
ui_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ui")
ui_dir = os.path.normpath(ui_dir)


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
        super().end_headers()

    def do_GET(self):
        """Handle GET requests — serve files or mock store API."""
        if self.path.startswith("/store/"):
            return self._handle_store_api()
        return super().do_GET()

    def _handle_store_api(self):
        """Mock store API for development."""
        import json as _json
        import urllib.parse

        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        query = urllib.parse.parse_qs(parsed.query)

        if path == "/store/catalog":
            kind = query.get("type", [None])[0]
            search = query.get("search", [None])[0]
            items = MOCK_CATALOG
            if kind:
                items = [i for i in items if i["type"] == kind]
            if search:
                s = search.lower()
                items = [i for i in items if s in i["name"].lower() or s in i.get("description", "").lower() or any(s in t for t in i.get("tags", []))]
            body = _json.dumps({"items": items, "total": len(items), "page": 1, "pageSize": 20})
            self._json_response(200, body)
            return

        if path.startswith("/store/catalog/"):
            parts = path.rstrip("/").split("/")
            # /store/catalog/<id>/download
            if len(parts) >= 5 and parts[-1] == "download":
                item_id = parts[-2]
                return self._handle_download(item_id)
            # /store/catalog/<id>
            item_id = parts[-1]
            item = next((i for i in MOCK_CATALOG if i["id"] == item_id), None)
            if not item:
                self._json_response(404, '{"error":"not found"}')
                return
            body = _json.dumps({**item, "readme": f"# {item['name']}\n\n{item.get('description','')}", "manifest": None, "size": 1024, "updatedAt": "2026-02-15T10:00:00Z"})
            self._json_response(200, body)
            return

        self._json_response(404, '{"error":"not found"}')

    def _handle_download(self, item_id):
        """Serve a .zip package for the given item ID."""
        zip_path = os.path.join(ui_dir, "store-packages", f"{item_id}.zip")
        if not os.path.isfile(zip_path):
            self._json_response(404, '{"error":"package not found"}')
            return
        try:
            with open(zip_path, "rb") as f:
                data = f.read()
            self.send_response(200)
            self.send_header("Content-Type", "application/zip")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Content-Disposition", f'attachment; filename="{item_id}.zip"')
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)
        except Exception as e:
            self._json_response(500, f'{{"error":"{e}"}}')

    def _json_response(self, code, body):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(body.encode())

    def log_message(self, format, *args):
        # Only log non-200 responses to reduce noise
        if len(args) >= 2 and str(args[1]) == "200":
            return
        super().log_message(format, *args)


# Mock store catalog data
MOCK_CATALOG = [
    {"id": "hello-world", "name": "Hello World", "type": "extension", "version": "1.0.0", "author": "kiro-assistant", "description": "Sample extension — type 'test' or 'hello' to see a greeting. Great starting template.", "icon": "👋", "downloads": 42, "rating": 5.0, "tags": ["sample", "template", "starter"]},
    {"id": "solarized-theme", "name": "Solarized", "type": "theme", "version": "1.0.0", "author": "community", "description": "Precision colors for machines and people", "icon": "🌅", "downloads": 2340, "rating": 4.7, "tags": ["dark", "light", "classic"]},
    {"id": "dracula-theme", "name": "Dracula", "type": "theme", "version": "1.0.0", "author": "community", "description": "A dark theme for code editors", "icon": "🧛", "downloads": 5120, "rating": 4.8, "tags": ["dark", "popular"]},
    {"id": "nord-theme", "name": "Nord", "type": "theme", "version": "1.0.0", "author": "community", "description": "Arctic, north-bluish color palette", "icon": "❄️", "downloads": 1890, "rating": 4.6, "tags": ["dark", "blue", "minimal"]},
    {"id": "web-dev-shortcuts", "name": "Web Dev Shortcuts", "type": "commands", "version": "1.0.0", "author": "community", "description": "Handy shortcuts for web developers — localhost, MDN, npm, caniuse", "icon": "🌐", "downloads": 890, "rating": 4.5, "tags": ["web", "developer", "shortcuts"]},
    {"id": "git-shortcuts", "name": "Git Shortcuts", "type": "commands", "version": "1.0.0", "author": "community", "description": "Quick commands for common git operations", "icon": "🔀", "downloads": 1200, "rating": 4.4, "tags": ["git", "developer", "shortcuts"]},
    {"id": "unit-converter", "name": "Unit Converter", "type": "extension", "version": "1.0.0", "author": "community", "description": "Convert between units — temperature, weight, distance, and more", "icon": "📏", "downloads": 670, "rating": 4.3, "tags": ["utility", "conversion"]},
    {"id": "password-gen", "name": "Password Generator", "type": "extension", "version": "1.0.0", "author": "community", "description": "Generate secure passwords and passphrases", "icon": "🔐", "downloads": 1450, "rating": 4.6, "tags": ["security", "utility"]},
]


http.server.HTTPServer.allow_reuse_address = True
server = http.server.HTTPServer(("", PORT), NoCacheHandler)
print(f"Dev server running on http://localhost:{PORT}")
sys.stdout.flush()
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()

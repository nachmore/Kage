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
        store_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "store", "packages")
        zip_path = os.path.join(store_dir, f"{item_id}.zip")
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
    {"id": "hello-world", "name": "Hello World", "type": "extension", "version": "1.0.0", "author": "kage", "description": "Sample extension — type 'test' or 'hello' to see a greeting. Great starting template.", "icon": "👋", "tags": ["sample", "template", "starter"]},
    {"id": "link-preview", "name": "Link Preview", "type": "extension", "version": "1.0.1", "author": "kage", "description": "Inline preview cards for URLs in AI responses — shows title, description, and favicon.", "icon": "🔗", "tags": ["utility", "formatting", "links"]},
    {"id": "color-picker", "name": "Color Picker", "type": "extension", "version": "1.0.0", "author": "kage", "description": "Detect and preview colors (hex, rgb, hsl, named) with format conversion.", "icon": "🎨", "tags": ["color", "design", "utility", "hex", "rgb"]},
    {"id": "dev-tools", "name": "Developer Tools", "type": "extension", "version": "1.0.0", "author": "kage", "description": "UUID generation, base64 encode/decode, hashing, epoch conversion, JSON formatting.", "icon": "🔧", "tags": ["developer", "uuid", "base64", "hash", "json", "utility"]},
    {"id": "timer", "name": "Timer & Stopwatch", "type": "extension", "version": "1.0.0", "author": "kage", "description": "Countdown timer and stopwatch with notification sounds.", "icon": "⏱️", "tags": ["timer", "stopwatch", "productivity", "pomodoro"]},
    {"id": "todos", "name": "Todos & Reminders", "type": "extension", "version": "1.0.0", "author": "kage", "description": "Task manager with due dates, categories, priorities, and progress tracking. Type 'todo' for a summary, 'todo+ <task>' to add, 'todo+ <task> due:<date>' for a reminder with a due-date banner.", "icon": "✅", "tags": ["productivity", "tasks", "todo", "organizer", "reminders"]},
    {"id": "dictionary", "name": "Dictionary", "type": "extension", "version": "1.7.2", "author": "kage", "description": "Look up word definitions, spelling corrections, and pronunciation. Supports 250+ languages via FreeDictionaryAPI.com.", "icon": "📖", "tags": ["dictionary", "spelling", "definitions", "language", "words", "translate"]},
    {"id": "focus-tracker", "name": "Focus Tracker", "type": "extension", "version": "1.3.2", "author": "kage", "description": "Track app usage, context switches, and focus streaks. Get daily/weekly/monthly reports with AI insights.", "icon": "📊", "tags": ["focus", "productivity", "screen time", "activity", "tracker", "heatmap"]},
    {"id": "nord-theme", "name": "Nord", "type": "theme", "version": "1.0.0", "author": "kage", "description": "Arctic, north-bluish color palette. Clean and icy.", "icon": "❄️", "tags": ["dark", "light", "blue", "minimal"]},
    {"id": "sunset-theme", "name": "Sunset", "type": "theme", "version": "1.0.0", "author": "kage", "description": "Warm sunset colors — amber accents with deep twilight backgrounds.", "icon": "🌅", "tags": ["dark", "light", "warm", "orange"]},
]


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

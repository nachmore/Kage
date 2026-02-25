"""Dev server for Tauri dev mode. Prints cwd for debugging, then serves ui/ on port 1420."""
import os
import sys
import signal
import socket
import subprocess
import platform

PORT = 1420

print(f"CWD: {os.getcwd()}")
print(f"ui exists: {os.path.isdir('ui')}")
print(f"ui/index.html exists: {os.path.isfile('ui/index.html')}")

# If just diagnosing, exit early
if "--check" in sys.argv:
    sys.exit(0)


def kill_existing_server():
    """Kill any process already listening on our port."""
    system = platform.system()
    try:
        if system == "Windows":
            # Find PID using netstat
            result = subprocess.run(
                ["netstat", "-ano", "-p", "TCP"],
                capture_output=True, text=True, timeout=5
            )
            for line in result.stdout.splitlines():
                if f":{PORT}" in line and "LISTENING" in line:
                    pid = int(line.strip().split()[-1])
                    if pid != os.getpid():
                        print(f"Killing existing server (PID {pid}) on port {PORT}")
                        subprocess.run(["taskkill", "/F", "/PID", str(pid)],
                                       capture_output=True, timeout=5)
        else:
            # macOS / Linux: use lsof
            result = subprocess.run(
                ["lsof", "-ti", f"tcp:{PORT}"],
                capture_output=True, text=True, timeout=5
            )
            for pid_str in result.stdout.strip().splitlines():
                pid = int(pid_str)
                if pid != os.getpid():
                    print(f"Killing existing server (PID {pid}) on port {PORT}")
                    os.kill(pid, signal.SIGKILL)
    except Exception as e:
        print(f"Warning: could not check for existing server: {e}")


kill_existing_server()

# Resolve ui/ directory as absolute path to avoid cwd issues
ui_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ui")
ui_dir = os.path.normpath(ui_dir)
print(f"Serving: {ui_dir}")

import http.server
import functools

handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=ui_dir)

http.server.HTTPServer.allow_reuse_address = True
server = http.server.HTTPServer(("", PORT), handler)
print(f"Dev server running on http://localhost:{PORT}")
server.serve_forever()

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

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, format, *args):
        # Only log non-200 responses to reduce noise
        if len(args) >= 2 and str(args[1]) == "200":
            return
        super().log_message(format, *args)


http.server.HTTPServer.allow_reuse_address = True
server = http.server.HTTPServer(("", PORT), NoCacheHandler)
print(f"Dev server running on http://localhost:{PORT}")
sys.stdout.flush()
server.serve_forever()

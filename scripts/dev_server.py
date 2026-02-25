"""Dev server for Tauri dev mode. Prints cwd for debugging, then serves ui/ on port 1420."""
import os
import sys

print(f"CWD: {os.getcwd()}")
print(f"ui exists: {os.path.isdir('ui')}")
print(f"ui/index.html exists: {os.path.isfile('ui/index.html')}")

# If just diagnosing, exit early
if "--check" in sys.argv:
    sys.exit(0)

# Resolve ui/ directory as absolute path to avoid cwd issues
ui_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ui")
ui_dir = os.path.normpath(ui_dir)
print(f"Serving: {ui_dir}")

import http.server
import functools

handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=ui_dir)
server = http.server.HTTPServer(("", 1420), handler)
print(f"Dev server running on http://localhost:1420")
server.serve_forever()

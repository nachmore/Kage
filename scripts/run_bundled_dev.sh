#!/bin/bash
# Launch the debug-bundled Kage.app with the dev server running.
#
# Usage:
#   ./scripts/run_bundled_dev.sh          # /dev /debug (default)
#   ./scripts/run_bundled_dev.sh --build  # rebuild first, then launch
#
# This solves the problem that `cargo tauri dev` runs an unbundled binary
# (missing macOS .app features like activation policy, TCC, Cmd+Tab),
# while running the .app directly skips the dev server that serves ui/.

set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Source .env for TAURI_SIGNING_PRIVATE_KEY (required by `cargo tauri build`)
if [ -f "$REPO_ROOT/.env" ]; then
  set -a
  source "$REPO_ROOT/.env"
  set +a
fi
APP_PATH="$REPO_ROOT/target/debug/bundle/macos/Kage.app/Contents/MacOS/Kage"
DEV_SERVER="$REPO_ROOT/scripts/dev_server.py"
PORT=1420

# --- Options ---
BUILD=false
EXTRA_ARGS=(/dev /debug)

for arg in "$@"; do
  case "$arg" in
    --build) BUILD=true ;;
    --no-debug) EXTRA_ARGS=(/dev) ;;
    *) EXTRA_ARGS+=("$arg") ;;
  esac
done

# --- Build if requested or binary missing ---
if [ "$BUILD" = true ] || [ ! -f "$APP_PATH" ]; then
  echo "🔨 Building debug bundle..."
  # Skip the separate MCP build in beforeBuildCommand. It causes a
  # double-compile: build_mcp.py runs `cargo build` without Tauri's
  # feature flags, then Tauri runs its own `cargo build` with different
  # features — invalidating the entire cache. The MCP binary gets built
  # and bundled by Tauri anyway (it's in externalBin / same target dir).
  KAGE_SKIP_MCP_BUILD=1 cargo tauri build --debug
fi

# --- Kill any existing Kage instance ---
if pgrep -f "Kage.app/Contents/MacOS/Kage" > /dev/null 2>&1; then
  echo "⏹  Stopping existing Kage..."
  pkill -f "Kage.app/Contents/MacOS/Kage" || true
  sleep 1
fi

# --- Start dev server if not already running ---
if ! lsof -ti tcp:$PORT > /dev/null 2>&1; then
  echo "🌐 Starting dev server on port $PORT..."
  python3 "$DEV_SERVER" --no-mcp-build &
  DEV_SERVER_PID=$!

  # Wait for it to be ready
  for i in $(seq 1 30); do
    if lsof -ti tcp:$PORT > /dev/null 2>&1; then
      break
    fi
    sleep 0.2
  done

  if ! lsof -ti tcp:$PORT > /dev/null 2>&1; then
    echo "❌ Dev server failed to start"
    kill $DEV_SERVER_PID 2>/dev/null || true
    exit 1
  fi
  echo "✅ Dev server ready"
else
  echo "🌐 Dev server already running on port $PORT"
  DEV_SERVER_PID=""
fi

# --- Cleanup on exit ---
cleanup() {
  if [ -n "$DEV_SERVER_PID" ]; then
    echo ""
    echo "⏹  Stopping dev server (PID $DEV_SERVER_PID)..."
    kill $DEV_SERVER_PID 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

# --- Launch Kage ---
echo "🚀 Launching Kage.app with: ${EXTRA_ARGS[*]}"
"$APP_PATH" "${EXTRA_ARGS[@]}"

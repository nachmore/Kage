#!/usr/bin/env bash
# Run `cargo tauri build "$@"`. Retry only if the bundler failed because a
# download returned a server error or timed out.
#
# The Windows bundler downloads NSIS and its plugins from GitHub release
# assets on a cold cache. When GitHub answers 503/504, the whole build fails
# with
#   Error failed to bundle project `http status: 503`
# even though the compile succeeded. Each retry reuses the compiled tree, so
# it only pays for bundling again. Compile errors, test failures and every
# other bundler error still fail on the first attempt.
#
# Env: TAURI_BUILD_ATTEMPTS (default 4).
set -uo pipefail

attempts="${TAURI_BUILD_ATTEMPTS:-4}"
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# A failure is transient only if the bundler reported it and the reason is
# a 5xx status, a timeout, or a dropped connection.
transient='failed to bundle project.*(http status: 5[0-9][0-9]|timed out|error sending request|connection (reset|closed|refused))'

status=1
for ((i = 1; i <= attempts; i++)); do
  cargo tauri build "$@" 2>&1 | tee "$log"
  status="${PIPESTATUS[0]}"
  if [ "$status" -eq 0 ]; then
    exit 0
  fi
  if ! grep -Eiq "$transient" "$log"; then
    exit "$status"
  fi
  if [ "$i" -lt "$attempts" ]; then
    wait_s=$((i * 30))
    echo "::warning::cargo tauri build: bundler download failed (attempt $i/$attempts); retrying in ${wait_s}s"
    sleep "$wait_s"
  fi
done

echo "::error::cargo tauri build: bundler download still failing after $attempts attempts"
exit "$status"
